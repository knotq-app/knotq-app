Folder icons are adapted from Zed's project icons:

- https://github.com/zed-industries/zed/blob/main/assets/icons/folder.svg
The copied SVGs replace hard-coded black fills/strokes with `currentColor` so GPUI can tint them with the active sidebar theme. The open-folder asset keeps the same 16px outline style but uses a lighter custom open state instead of Zed's filled wedge.

Other SVGs in this directory are small outline app icons using the same `currentColor` convention so GPUI `IconName` references resolve from the app asset bundle.
