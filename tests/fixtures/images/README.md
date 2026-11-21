Image decoder fixtures contain synthetic solid colors, 16×12 px. `red.png` asks for
`#F0140A` and `blue.png` for `#0A14F0`; FFmpeg 8.0 passes them through YUV, so
the red files decode to `#EE1309`:

```sh
ffmpeg -f lavfi -i "color=c=0xF0140A:s=16x12" -frames:v 1 red.png
ffmpeg -f lavfi -i "color=c=0x0A14F0:s=16x12" -frames:v 1 blue.png
```

The fixtures were generated from them with FFmpeg 8.0 and libwebp 1.6.0:

```sh
cwebp -q 95 red.png -o lossy.webp
cwebp -lossless red.png -o lossless.webp
img2webp -lossless -d 100 red.png blue.png -o animated.webp
cp red.png f1.png && cp blue.png f2.png
ffmpeg -framerate 2 -i f%d.png \
  -vf "split[a][b];[a]palettegen=reserve_transparent=0[p];[b][p]paletteuse" \
  -loop 0 animated.gif
ffmpeg -i red.png -pix_fmt bgr24 rgb24.bmp
```

ICO and SVG test inputs are built in the tests themselves.
