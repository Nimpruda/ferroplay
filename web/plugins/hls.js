// HLS playback: `player(video, { hls }).play('https://…/master.m3u8')`. To tune it, `hlsWith`
// (below) takes functions of the target duration, e.g. `hlsWith({ ahead: target => 2 * target })`.
// Browsers with built-in HLS need nothing: the URL becomes `video.src`. Elsewhere this turns on
// the Rust `hls` plugin, which decides everything; this file only carries out its commands:
// downloads and Media Source calls.
export function hls({ video, send, use, handle, seekable }) {
  if (video.canPlayType('application/vnd.apple.mpegurl') || !window.MediaSource) return;
  const text = bytes => new TextDecoder().decode(bytes);
  let download = new AbortController();
  let source, buffer, objectUrl, queue = [];

  // A delay that a newer download cancels by aborting its signal.
  const wait = (ms, signal) => new Promise((resolve, reject) => {
    const timer = setTimeout(resolve, ms);
    signal.addEventListener('abort', () => { clearTimeout(timer); reject(signal.reason); }, { once: true });
  });
  // The end of a seekable range travels as 8 little-endian bytes, the same as `f64::to_le_bytes` in Rust.
  const fromBytes = bytes => new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getFloat64(0, true);

  // A Media Source takes one change at a time: queue them, run the next one when it is free.
  const pump = () => {
    while (queue.length && source?.readyState !== 'closed' && !buffer?.updating) {
      try { queue.shift()(); } catch (e) { console.error(e); }
    }
  };
  const later = change => { queue.push(change); pump(); };

  handle({
    async fetch(delay, bytes) {
      download.abort(); // one download at a time: a new one replaces any left over from a previous URL, and its wait
      const { signal } = download = new AbortController();
      let start, data;
      try {
        if (delay > 0) await wait(delay * 1000, signal);
        start = performance.now(); // the download clock starts once the wait is over
        const response = await fetch(text(bytes), { signal });
        if (!response.ok) throw new Error(`HTTP ${response.status} for ${response.url}`);
        data = new Uint8Array(await response.arrayBuffer());
      } catch (e) {
        if (!signal.aborted) {
          console.warn(e);
          send('loadFailed');
        }
        return;
      }
      // Outside the try: a failure in the core is a bug, not a failed download, so it must not be swallowed here.
      send('loaded', performance.now() - start, data);
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
    seekableRange: (start, bytes) => later(() => {
      const end = fromBytes(bytes);
      // setLiveSeekableRange throws on anything but finite times with start <= end: say so, and keep playing.
      if (!(Number.isFinite(start) && Number.isFinite(end) && start >= 0 && start <= end)) {
        return console.warn(`seekable range ${start}–${end} ignored`);
      }
      try { source.setLiveSeekableRange(start, end); } catch (e) { return console.warn(e); }
      seekable(); // no event fires for the change: report it now
    }),
  });
  use('hls');

  return () => {
    download.abort();
    URL.revokeObjectURL(objectUrl);
  };
}

// `hls` with tuned values. Each knob is a function returning its value: `ahead`, `behind` and `holdback`
// get the target duration (seconds) and return seconds; `safety` (share of bandwidth) and the retries get
// nothing. A knob left out keeps its default: 30 s ahead and behind, a hold-back of three target durations,
// 0.8 of the bandwidth, and retries after a failure before giving up: 3 (`retries`), 10 for a live stream
// (`liveRetries`), and 0 after an init section this player cannot play (`initRetries`).
export const hlsWith = knobs => api => {
  for (const [name, knob] of Object.entries(knobs)) api.tune(name, knob);
  return hls(api);
};
