// HLS playback: `player(video, { hls }).play('https://…/master.m3u8')`.
// Browsers with built-in HLS need nothing: the URL becomes `video.src`. Elsewhere this turns on
// the Rust `hls` plugin, which decides everything; this file only carries out its commands:
// downloads and Media Source calls.
export function hls({ video, send, use, handle }) {
  if (video.canPlayType('application/vnd.apple.mpegurl') || !window.MediaSource) return;
  const text = bytes => new TextDecoder().decode(bytes);
  let download = new AbortController();
  let source, buffer, objectUrl, queue = [];

  // A Media Source takes one change at a time: queue them, run the next one when it is free.
  const pump = () => {
    while (queue.length && source?.readyState !== 'closed' && !buffer?.updating) {
      try { queue.shift()(); } catch (e) { console.error(e); }
    }
  };
  const later = change => { queue.push(change); pump(); };

  handle({
    async fetch(_, bytes) {
      download.abort(); // one download at a time: a new one replaces any left over from a previous URL
      const { signal } = download = new AbortController();
      const start = performance.now();
      try {
        const response = await fetch(text(bytes), { signal });
        if (!response.ok) throw new Error(`HTTP ${response.status} for ${response.url}`);
        const data = new Uint8Array(await response.arrayBuffer());
        send('loaded', performance.now() - start, data);
      } catch (e) {
        if (!signal.aborted) {
          console.warn(e);
          send('loadFailed');
        }
      }
    },
    open(duration) {
      // A new source restarts the element: start a fresh queue, and keep playing if asked to.
      const playing = !video.paused;
      const ms = source = new MediaSource();
      queue = [];
      buffer = undefined;
      ms.addEventListener('sourceopen', () => { ms.duration = duration; pump(); }, { once: true });
      URL.revokeObjectURL(objectUrl);
      video.src = objectUrl = URL.createObjectURL(ms);
      if (playing) video.play().catch(() => {});
    },
    codec: (_, bytes) => later(() => {
      if (buffer) buffer.changeType(text(bytes));
      else (buffer = source.addSourceBuffer(text(bytes))).addEventListener('updateend', pump);
    }),
    append: (_, bytes) => later(() => buffer.appendBuffer(bytes)),
    remove: end => later(() => buffer.remove(0, end)),
    end: () => later(() => source.readyState === 'open' && source.endOfStream()),
  });
  use('hls');

  return () => {
    download.abort();
    URL.revokeObjectURL(objectUrl);
  };
}
