# Media Source traps we hit

- **A seek into unbuffered data fires `seeking` but no `timeupdate`** until data arrives. Report the time on
  `seeking` too, or the engine never learns where to load from.
- **`remove()` or `appendBuffer()` reopens an ended Media Source.** Call `endOfStream()` again whenever the
  last segment is in, or playback never reaches `ended`.
- **One change at a time:** queue appends, removes and `changeType`, and run the next on `updateend`.
- **Setting `video.src` restarts the element and pauses it.** When swapping in a new Media Source, call
  `play()` again if it was playing.
- **`progress` may not fire for Media Source:** recompute the buffered end on `timeupdate`.
- **Quality switches change the codec string:** `changeType(mime)` plus a new init segment.
