---
name: website-release
description: Validate and prepare KnotQ website changes or deployment without publishing unexpectedly.
---

Use for website content, localization, hosting, or deployment work. Inspect
`.openai/hosting.json` and the site's scripts first, then run the narrowest
available build/link/localization checks. Validate changed HTML/JS, locale-key
parity, redirects, canonical URLs, and asset paths. Keep deployment separate:
do not invoke a hosting publish command, invalidate caches, or alter DNS without
explicit authorization. Record the exact preview/build artifact and any check
that could not run locally.
