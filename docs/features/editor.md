# Annotation editor

Status: **planned**.

Opened from the preview card, `after_capture.edit`, or `screenie edit FILE`.

## Tools

Arrow, line, rectangle, ellipse, pen, highlighter, text, numbered step, pixelate/blur,
spotlight, crop. Each shape stays editable (select, move, restyle) until export. It's
an object canvas, not a paint program.

## Architecture

- `screenie-annotate`: the document model (a base image and a list of shapes with
  style) and a **tiny-skia** renderer used for export and the canvas cache. It has no
  UI dependencies, so export is testable headlessly.
- `screenie-editor`: the GPUI window. It has a toolbar, a colour palette
  (`editor.palette`), undo/redo, and Copy / Save / Save As.
