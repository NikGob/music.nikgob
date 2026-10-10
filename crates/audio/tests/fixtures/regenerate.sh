#!/bin/sh
# Regenerates the real-encoder fixtures (needs ffmpeg with libopus, libmp3lame,
# libvorbis). Tests never run this; the files are committed.
# Content: left = 0.3*sin(2*pi*440*t), right = 0.25*sin(2*pi*660*t), where t
# continues across opus_a / opus_b (consecutive pieces of one recording).
set -e
cd "$(dirname "$0")"
gen() { # offset duration encoder-args...
  off=$1; dur=$2; shift 2
  ffmpeg -hide_banner -loglevel error -y -f lavfi \
    -i "aevalsrc=0.3*sin(2*PI*440*(t+$off))|0.25*sin(2*PI*660*(t+$off)):s=44100:d=$dur" "$@"
}
gen 0 1.5 -c:a libopus -b:a 64k -ar 48000 opus_a.opus
gen 1.5 1.0 -c:a libopus -b:a 64k -ar 48000 opus_b.opus
gen 0 1.0 -c:a libmp3lame -b:a 128k tone.mp3
gen 0 1.0 -c:a libvorbis -q:a 3 tone.ogg
gen 0 1.0 -c:a aac -b:a 96k tone.m4a
gen 0 0.5 -c:a alac tone_alac.m4a
gen 0 0.5 -c:a flac tone.flac
