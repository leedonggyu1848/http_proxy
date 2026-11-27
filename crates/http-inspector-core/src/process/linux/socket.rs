//! Finds the client endpoint, reversing the listener's accepted tuple.

use std::fs;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use http_inspector_exchange::ProcessUnavailable;
use rustix::net::netlink::{SOCK_DIAG, SocketAddrNetlink};
use rustix::net::{
    AddressFamily, RecvFlags, SendFlags, SocketFlags, SocketType, bind, recvfrom, sendto,
    socket_with,
    sockopt::{Timeout, set_socket_timeout},
};

const DIAG_BY_FAMILY: u16 = 20;
const NLM_F_REQUEST: u16 = 1;
const INET_DIAG_NOCOOKIE: [u8; 8] = [0xff; 8];
const NETLINK_HEADER_BYTES: usize = 16;
const DIAG_MESSAGE_BYTES: usize = 72;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Owner {
    pub(super) uid: u32,
    pub(super) inode: u64,
}

pub(super) fn find(peer: SocketAddr, proxy: SocketAddr) -> Result<Owner, ProcessUnavailable> {
    if let Some(owner) = query_diag(peer, proxy) {
        return Ok(owner);
    }

    let (mut any_table, mut denied) = (false, false);
    for (path, ipv6) in [("/proc/net/tcp", false), ("/proc/net/tcp6", true)] {
        match fs::read_to_string(path) {
            Ok(table) => {
                any_table = true;
                if let Some(owner) = table_owner(&table, ipv6, peer, proxy) {
                    return Ok(owner);
                }
            }
            Err(error) => denied |= error.kind() == io::ErrorKind::PermissionDenied,
        }
    }
    if denied && !any_table {
        Err(ProcessUnavailable::PermissionDenied)
    } else {
        Err(ProcessUnavailable::SocketUnavailable)
    }
}

fn query_diag(peer: SocketAddr, proxy: SocketAddr) -> Option<Owner> {
    let request = diag_request(peer, proxy)?;
    let socket = socket_with(
        AddressFamily::NETLINK,
        SocketType::RAW,
        SocketFlags::CLOEXEC,
        Some(SOCK_DIAG),
    )
    .ok()?;
    let kernel = SocketAddrNetlink::new(0, 0);
    bind(&socket, &kernel).ok()?;
    set_socket_timeout(&socket, Timeout::Recv, Some(Duration::from_millis(20))).ok()?;
    let sent = sendto(&socket, &request, SendFlags::DONTWAIT, &kernel).ok()?;
    if sent != request.len() {
        return None;
    }
    let mut response = [0; 256];
    let (received, size, from) = recvfrom(&socket, &mut response[..], RecvFlags::empty()).ok()?;
    let from = SocketAddrNetlink::try_from(from?).ok()?;
    (from == kernel && received == size)
        .then(|| response.get(..received).and_then(decode_diag))
        .flatten()
}

fn diag_request(peer: SocketAddr, proxy: SocketAddr) -> Option<Vec<u8>> {
    let (family, source, destination) = match (peer.ip(), proxy.ip()) {
        (IpAddr::V4(peer), IpAddr::V4(proxy)) => {
            let padded = |address: Ipv4Addr| {
                let mut bytes = [0; 16];
                if let Some(start) = bytes.get_mut(..4) {
                    start.copy_from_slice(&address.octets());
                }
                bytes
            };
            (AddressFamily::INET, padded(peer), padded(proxy))
        }
        (IpAddr::V6(peer), IpAddr::V6(proxy)) => {
            (AddressFamily::INET6, peer.octets(), proxy.octets())
        }
        _ => return None,
    };
    // nlmsghdr followed by inet_diag_req_v2 and its exact inet_diag_sockid.
    let mut request = Vec::with_capacity(DIAG_MESSAGE_BYTES);
    request.extend_from_slice(&72_u32.to_ne_bytes());
    request.extend_from_slice(&DIAG_BY_FAMILY.to_ne_bytes());
    request.extend_from_slice(&NLM_F_REQUEST.to_ne_bytes());
    request.extend_from_slice(&[0; 8]); // sequence number and sending port ID
    request.extend_from_slice(&[u8::try_from(family.as_raw()).ok()?, 6, 0, 0]); // TCP, no extensions
    request.extend_from_slice(&u32::MAX.to_ne_bytes()); // all TCP states
    request.extend_from_slice(&peer.port().to_be_bytes());
    request.extend_from_slice(&proxy.port().to_be_bytes());
    request.extend_from_slice(&source);
    request.extend_from_slice(&destination);
    request.extend_from_slice(&[0; 4]); // no bound interface
    request.extend_from_slice(&INET_DIAG_NOCOOKIE);
    Some(request)
}

fn decode_diag(response: &[u8]) -> Option<Owner> {
    let message_type = u16::from_ne_bytes(response.get(4..6)?.try_into().ok()?);
    let length = usize::try_from(u32::from_ne_bytes(response.get(..4)?.try_into().ok()?)).ok()?;
    if message_type != DIAG_BY_FAMILY || length < NETLINK_HEADER_BYTES + DIAG_MESSAGE_BYTES {
        return None;
    }
    let message = response.get(NETLINK_HEADER_BYTES..length)?;
    let uid = u32::from_ne_bytes(message.get(64..68)?.try_into().ok()?);
    let inode = u32::from_ne_bytes(message.get(68..72)?.try_into().ok()?);
    (inode != 0).then_some(Owner {
        uid,
        inode: u64::from(inode),
    })
}

fn table_owner(table: &str, ipv6: bool, peer: SocketAddr, proxy: SocketAddr) -> Option<Owner> {
    table.lines().skip(1).find_map(|line| {
        let mut columns = line.split_ascii_whitespace().skip(1);
        let source = table_address(columns.next()?, ipv6)?;
        let destination = table_address(columns.next()?, ipv6)?;
        if source != peer || destination != proxy {
            return None;
        }
        let uid = columns.nth(4)?.parse().ok()?;
        let inode = columns.nth(1)?.parse().ok()?;
        (inode != 0).then_some(Owner { uid, inode })
    })
}

fn table_address(value: &str, ipv6: bool) -> Option<SocketAddr> {
    let (address, port) = value.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    let ip = if ipv6 {
        if address.len() != 32 {
            return None;
        }
        // /proc prints IPv6 as four native-endian u32 words.
        let mut octets = [0; 16];
        for (word, chunk) in address
            .as_bytes()
            .as_chunks::<8>()
            .0
            .iter()
            .zip(octets.as_chunks_mut::<4>().0.iter_mut())
        {
            let word = std::str::from_utf8(word).ok()?;
            chunk.copy_from_slice(&u32::from_str_radix(word, 16).ok()?.to_ne_bytes());
        }
        Ipv6Addr::from(octets).to_canonical()
    } else {
        if address.len() != 8 {
            return None;
        }
        IpAddr::V4(Ipv4Addr::from(
            u32::from_str_radix(address, 16).ok()?.to_ne_bytes(),
        ))
    };
    Some(SocketAddr::new(ip, port))
}

#[cfg(test)]
mod tests {
    use super::{Owner, query_diag, table_owner};
    use std::fmt::Write as _;
    use std::net::{Ipv4Addr, TcpListener, TcpStream};

    #[test]
    fn selects_the_client_tuple_and_ignores_closed_rows() {
        let word = format!("{:08X}", u32::from_ne_bytes(Ipv4Addr::LOCALHOST.octets()));
        let table = format!(
            "header\n 0: {word}:1F90 {word}:9C40 01 0:0 0:0 0 1000 0 91\n 1: {word}:9C40 {word}:1F90 06 0:0 0:0 0 1000 0 0\n 2: {word}:9C40 {word}:1F90 01 0:0 0:0 0 1000 0 92"
        );
        assert_eq!(
            table_owner(
                &table,
                false,
                ([127, 0, 0, 1], 40000).into(),
                ([127, 0, 0, 1], 8080).into()
            ),
            Some(Owner {
                inode: 92,
                uid: 1000
            })
        );
    }

    #[test]
    fn decodes_ipv4_mapped_ipv6_in_the_fallback_table() {
        let raw = Ipv4Addr::LOCALHOST.to_ipv6_mapped().octets();
        let mut word = String::new();
        for &bytes in raw.as_chunks::<4>().0 {
            let _written = write!(word, "{:08X}", u32::from_ne_bytes(bytes));
        }
        let table = format!("header\n 0: {word}:9C40 {word}:1F90 01 0:0 0:0 0 1000 0 92");
        assert_eq!(
            table_owner(
                &table,
                true,
                ([127, 0, 0, 1], 40000).into(),
                ([127, 0, 0, 1], 8080).into()
            ),
            Some(Owner {
                inode: 92,
                uid: 1000
            })
        );
    }

    #[test]
    fn netlink_reports_the_client_socket() -> std::io::Result<()> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let proxy = listener.local_addr()?;
        let client = TcpStream::connect(proxy)?;
        let peer = client.local_addr()?;
        let (_accepted, _) = listener.accept()?;
        assert!(
            query_diag(peer, proxy)
                .is_some_and(|owner| owner.uid == rustix::process::geteuid().as_raw())
        );
        Ok(())
    }
}
