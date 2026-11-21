#!/bin/sh
# Assemble screen.html from src/ and build the canvas.
set -e
cd "$(dirname "$0")"
{
  echo '<template id="uh-screen">'
  echo '<style>'; cat src/1-style.css src/2-extra.css; echo '</style>'
  cat src/2-sections.html
  echo '</template>'
  echo '<script>'; cat src/3-data.js src/4-core.js src/5-pages.js src/6-panel.js
  echo 'window.ultrahandSetup = function (c) { setup(c); };'
  echo '</script>'
} > screen.html
UH="${UH:-$HOME/.claude/plugins/cache/mrgnhnt96/ultrahand/0.11.0}"
node "$UH/shared/canvas/build.mjs" --presets "$UH/skills/desktop/references/presets.md" \
  --config config.json --screen screen.html --title "Taskboard redesign" --out index.html
