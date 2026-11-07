# $wenlan theme

You design the theme; Wenlan checks it and applies it. Be creative with the
user's idea, but every value must pass Wenlan's checks, and nothing is applied
until it does.

## 1. Find the folder

Call `theme(action="list")`. It returns `themes_dir`, the themes already
there, and which one is in use. To change an existing theme, read its
`theme.json` first and edit it in place.

## 2. Write the theme

Pick an id: lowercase letters, digits and dashes, such as `night-bunny`.
Write `<themes_dir>/<id>/theme.json`:

```json
{
  "schema": 1,
  "name": "Night Bunny",
  "base": "dark",
  "author": "<this AI tool's name>",
  "colors": {
    "background": "#15101E",
    "surface": "#1F1729",
    "sidebar": "#120D1A",
    "text": "#F3ECF8",
    "textSecondary": "#CDBFD9",
    "textTertiary": "#9C8CAD",
    "border": "#352841",
    "accent": "#F29AC9",
    "textOnAccent": "#1A0F1F",
    "accentWarm": "#B99AE6",
    "link": "#E7A6D2"
  },
  "fonts": { "heading": "Fraunces", "body": "Instrument Sans" },
  "radius": 1.4,
  "images": {
    "background": { "file": "stars.png", "placement": "sidebar", "opacity": 0.35 },
    "sticker": { "file": "bunny.png", "corner": "bottom-right", "size": 88 }
  }
}
```

Rules the checks enforce:

- `base` is `dark` or `light`. Every color you leave out keeps that base's
  built-in value, so a theme can set only two or three colors.
- Colors are `#RRGGBB` only. Color slots: `background`, `surface`, `card`,
  `sidebar`, `text`, `textSecondary`, `textTertiary`, `border`, `accent`,
  `textOnAccent`, `accentWarm`, `link`, `danger`, `warning`, `success`, and the
  map colors `graphProject`, `graphTool`, `graphOrg`, `graphPerson`,
  `graphConcept`, `graphMemory`, `graphPage`, `graphEdge`.
- Text must stay readable: `text` and `textSecondary` need 4.5:1 contrast on
  the backgrounds, `textTertiary` 3:1, and `textOnAccent` 4.5:1 on `accent`.
  Pick a darker background or a lighter text before giving up on an idea.
- Fonts come from a fixed list (check reports it): Fraunces, Instrument Sans,
  JetBrains Mono, Georgia, Times New Roman, Palatino, Helvetica Neue, Arial,
  Verdana, Menlo, Courier New, PingFang TC/SC, Noto Sans/Serif CJK TC, and
  `serif`, `sans-serif`, `monospace`, `system-ui`. Nothing is downloaded.
- `radius` is 0 (square corners) to 2.5; 1 is Wenlan's default.
- Images are PNG, JPEG or WebP files placed directly in the theme folder, named
  without a path. No SVG. Background at most 5 MB, sticker at most 1 MB,
  sticker size 48 to 160. `placement` is `sidebar` (keeps reading text on a
  plain surface; prefer it) or `window`. `corner` is `bottom-right`,
  `bottom-left` or `top-right`. Images are optional.
- `advanced` may set any Wenlan CSS token directly (`"--kg-person": "#F2A7D0"`)
  when a slot does not cover it. Prefer the slots.

Images: use files the user gives you or files you can make yourself (for
example a small star pattern drawn with a script). Do not copy a brand's or a
character's artwork; when the user names a character, make an original look
in that spirit and say so.

## 3. Check, fix, apply

1. `theme(action="check", id="<id>")`. Fix every problem it lists in
   `theme.json` and check again. Never apply a theme that fails.
2. `theme(action="apply", id="<id>")`. The app switches within a few seconds.
3. Tell the user in one or two sentences what you made, and that they can go
   back with `$wenlan theme reset` or with "Back to default" in the app's
   Appearance settings.

`theme(action="reset")` goes back to the built-in themes.
