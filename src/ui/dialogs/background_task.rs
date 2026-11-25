use std::io;
use std::sync::mpsc::{self, Receiver, SendError, Sender, TryRecvError};
use std::thread;

use eframe::egui;

pub(super) enum TaskPoll<T> {
    Pending,
    Ready(io::Result<T>),
    Disconnected,
}

pub(super) struct BackgroundTask<T>(Receiver<io::Result<T>>);

impl<T> BackgroundTask<T> {
    pub(super) const fn track(receiver: Receiver<io::Result<T>>) -> Self {
        Self(receiver)
    }

    pub(super) fn poll(&self) -> TaskPoll<T> {
        match self.0.try_recv() {
            Ok(result) => TaskPoll::Ready(result),
            Err(TryRecvError::Empty) => TaskPoll::Pending,
            Err(TryRecvError::Disconnected) => TaskPoll::Disconnected,
        }
    }
}

impl<T: Send + 'static> BackgroundTask<T> {
    pub(super) fn start(
        name: &str,
        context: &egui::Context,
        work: impl FnOnce() -> io::Result<T> + Send + 'static,
    ) -> Self {
        let (task, starter) = Self::deferred();
        starter.start(name, context, work);
        task
    }

    /// Lets egui mark the task pending before its screenshot callback can run.
    pub(super) fn deferred() -> (Self, TaskStarter<T>) {
        let (sender, receiver) = mpsc::channel();
        (Self::track(receiver), TaskStarter(sender))
    }
}

pub(super) struct TaskStarter<T>(Sender<io::Result<T>>);

impl<T: Send + 'static> TaskStarter<T> {
    pub(super) fn start(
        self,
        name: &str,
        context: &egui::Context,
        work: impl FnOnce() -> io::Result<T> + Send + 'static,
    ) {
        self.start_reporting(name, context, |_| work());
    }

    /// Allows the worker to report an intermediate value before its final result.
    pub(super) fn start_reporting(
        self,
        name: &str,
        context: &egui::Context,
        work: impl FnOnce(&dyn Fn(T) -> Result<(), SendError<()>>) -> io::Result<T> + Send + 'static,
    ) {
        self.start_with(context, work, |worker| {
            thread::Builder::new().name(name.to_owned()).spawn(worker)?;
            Ok(())
        });
    }

    fn start_with(
        self,
        context: &egui::Context,
        work: impl FnOnce(&dyn Fn(T) -> Result<(), SendError<()>>) -> io::Result<T> + Send + 'static,
        spawn: impl FnOnce(Box<dyn FnOnce() + Send>) -> io::Result<()>,
    ) {
        let sender = self.0.clone();
        let worker_context = context.clone();
        if let Err(error) = spawn(Box::new(move || {
            let result = work(&|value| sender.send(Ok(value)).map_err(|_| SendError(())));
            let _receiver_closed = sender.send(result);
            worker_context.request_repaint();
        })) {
            let _receiver_closed = self.0.send(Err(error));
            context.request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{BackgroundTask, TaskPoll};

    #[tokio::test]
    async fn polls_the_result_of_a_worker() -> std::io::Result<()> {
        let context = eframe::egui::Context::default();
        let task = BackgroundTask::start("test-background-task", &context, || Ok(42));
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match task.poll() {
                TaskPoll::Ready(result) => {
                    assert_eq!(result?, 42);
                    return Ok(());
                }
                TaskPoll::Pending if Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
                TaskPoll::Pending => return Err(std::io::Error::other("test worker timed out")),
                TaskPoll::Disconnected => {
                    return Err(std::io::Error::other("test worker disconnected"));
                }
            }
        }
    }

    #[test]
    fn polls_a_spawn_failure_as_a_result() {
        let (task, starter) = BackgroundTask::deferred();
        let context = eframe::egui::Context::default();
        starter.start_with(
            &context,
            |_| Ok(()),
            |_| Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, "test")),
        );
        assert!(matches!(
            task.poll(),
            TaskPoll::Ready(Err(error)) if error.kind() == std::io::ErrorKind::WouldBlock
        ));
    }

    #[test]
    fn distinguishes_a_pending_task_from_a_dropped_sender() {
        let (task, starter) = BackgroundTask::<()>::deferred();
        assert!(matches!(task.poll(), TaskPoll::Pending));
        drop(starter);
        assert!(matches!(task.poll(), TaskPoll::Disconnected));
    }
}
