# Local changes to gpui-base 0.7.1

Vendored from crates.io and patched in through the workspace's `[patch.crates-io]`.

- `src/text_selection.rs`: holding the button down after a double or triple click and dragging
  extends the selection by whole words or lines, the way macOS text does. The first word or
  line stays selected whichever way the drag goes. Before, a multi-click ended the gesture, so
  a drag after it selected nothing. (`granular_drag`, `begin_granular_drag`,
  `snap_granular_drag`, `text_endpoint`.)
