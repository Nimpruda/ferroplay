# Live streams in Media Source

Measured 2026-10-15 in headless Firefox 130 (Linux), with five throwaway Media Source pages driven through
`e2e/browser.py`'s Firefox driver. The media was a fragmented MP4 from `ts2mp4` on `fixtures/sample.ts`
(one 2.07 s fragment). Each value is what the page read back. This checks the live design.

## What Firefox does

| Claim in the design | Result |
| --- | --- |
| `duration = Infinity` opens a live source | Accepted, before and after data. `video.duration` is `Infinity`. No event fires. |
| `setLiveSeekableRange(start, end)` exists and takes effect at once | Yes. `video.seekable` shows the new range on the next line, with or without data buffered. |
| `video.seekable` reports the window | **Only partly.** It reports the union of the window and the buffered ranges (see below). |
| The browser clamps seeks to the window | **No.** Seeking to 1 s with window [3, 5.5] landed at 1 s. Seeking to 6 s with window end 5.5 landed at 6 s. |
| A window change fires an event | **No.** `setLiveSeekableRange` and `clearLiveSeekableRange` fire nothing. Appends fire `progress`, and the first append also fires `durationchange` and `loadedmetadata`. |
| Bad windows are rejected | `TypeError` for `start > end`, a negative start, a NaN, and an `Infinity` end. |
| `clearLiveSeekableRange()` removes the window | Yes. `video.seekable` goes back to the buffered ranges. |
| A VOD source reports `[0, duration]` | Yes. With duration set to the buffered end (2.087 s), `seekable` is `[0, 2.087]`. `endOfStream()` gives `readyState` `ended`. |
| Aborting a delayed fetch cancels it | Cancelling the timer with `clearTimeout` before it fires: the server log has no request for it. Aborting a fetch right after calling it: `AbortError`, and the server log has no request. Aborting mid-download is not measured here. |

### The seekable range is a union

Measured with buffered ranges and windows set one after another:

| Buffered | Window | `video.seekable` |
| --- | --- | --- |
| [0.067, 2.067] | [0, 2.067] | [0, 2.067] |
| [0.067, 2.067] | [2, 32.067] | [0.067, 32.067] |
| [0.067, 6.201] (three fragments) | [1, 4] | [0.067, 6.201] |
| [2.134, 6.201] (first fragment removed) | [3, 6] | [2.134, 6.201] |
| [2.134, 6.201] | [0.5, 6] | [0.5, 6.201] |
| [2.134, 6.201] | [2.2, 3.0] | [2.134, 6.201] |

So `seekable` starts at the earlier of the window start and the first buffered point, and ends at the later of
the window end and the last buffered point. Both ends point at data or at the playlist, so seeking to them is
safe. The window alone is not what the browser reports.

## Design corrections

These correct the live design as first written.

1. **`Fact::Seekable` is the browser's `video.seekable`, as the design says, but do not expect it to equal the
   window.** Tests must assert the union. The core clamps seeks itself, because the browser does not clamp to the
   window.
2. **The host must report `Fact::Seekable` itself.** No event fires when the window changes. Read
   `video.seekable` right after applying `Cmd::SeekableRange`, and again on each `progress` event. Removals were
   not measured: check whether they fire anything before relying on the event alone.
3. **The host must validate the range before it calls the browser.** Only finite values with
   `0 <= start <= end` are accepted. Send nothing with `Infinity` as the end. Catch the `TypeError` and report
   it, so a bad window does not stop playback.
4. **Delay and abort work as the design says.** A timer that starts the download, cleared by a newer fetch, sends
   no request.

## VOD composability

- The same `Fact::Seekable` covers VOD: a finite duration gives `[0, duration]`, with no `SeekableRange` command.
- VOD never sends `Cmd::SeekableRange`, and its `Cmd::Fetch` has no delay.
- `Cmd::End` and `Cmd::Remove` are the same for VOD and live. Only the live stream sends the window and the reloads.

## Not checked here

- Native HLS live in Safari, and in Chrome: this run is Firefox only.
- Whether `sb.remove()` fires an event (the host needs it to re-read `seekable`).
- A mid-download abort (the abort above happens before the request reaches the server).
