# Performance notes

Keep measurements and optimizations tied to a user-visible scenario. A faster
probe that skips content, moves work onto the main actor, or invalidates sync
state is a regression.

## iOS editor images

`EditorTextView` paints image blocks from `drawRect`, so loading a camera-sized
file directly there can cause a decode hitch and retain far more memory than
the visible block needs. The renderer uses ImageIO thumbnails keyed by path
and display pixel size, with an `NSCache` budget of 32 entries / 48 MiB per
editor and a full-file fallback for unsupported formats. Draw-time cache misses
paint the existing placeholder and schedule one bounded background decode per
key; the editor invalidates its display after the thumbnail arrives. The
regression tests are `KeyboardAccessoryTests.testEditorImageBlocksDownsampleAndBoundTheirCache`
and `KeyboardAccessoryTests.testEditorImageDrawMissDecodesOffMainAndEventuallyRepaints`.

When changing this path, verify both a large image and an unsupported/corrupt
file. Preserve the stored media path and the model-reported dimensions; only
the in-memory drawing representation should change.

## Current evidence

- iOS simulator full suite: 165 tests passed with zero failures or skips after
  the asynchronous image scheduling coverage was added.
- The simulator still emits OS-owned accessibility and background-task
  diagnostics; these are not app assertions. UIKit test-window warnings should
  not be used as production crash evidence without reproducing from an app
  archive on a real device.
