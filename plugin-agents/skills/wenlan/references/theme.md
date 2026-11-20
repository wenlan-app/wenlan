# /wenlan theme

You design the theme; Wenlan checks it and applies it. Be creative with the
user's idea, but every value must pass Wenlan's checks, and nothing is applied
until it does.

## 1. Colors, fonts and corners: one call

Pick an id: lowercase letters, digits and dashes, such as `purple` or
`night-bunny`. Then save and apply in one call:

```
theme(action="apply", id="purple", theme={"name": "Purple", "base": "dark",
  "colors": {"background": "#1E1530", "surface": "#271C3D", "sidebar": "#181026",
             "accent": "#B794F4", "link": "#C9A9F7"}})
```

Set only the colors the idea needs. Wenlan fills in the file's bookkeeping
(`schema`, and `author` as this AI tool), keeps every text color you leave out
readable on your colors, and moves a color that is only just too faint a
little, saying which. When it still refuses, each problem comes back with a
color to try: fix those and call again. `theme(action="list")` shows the
themes there are and which one is in use; `theme(action="save", ...)` saves
without applying.

## 2. With a background image or a sticker

Images are files, so this one goes through the themes folder. Call
`theme(action="list")` for `themes_dir`, put the images and
`<themes_dir>/<id>/theme.json` there, then run
`theme(action="check", id="<id>")` until it passes, and
`theme(action="apply", id="<id>")`. To change an existing theme, read its
`theme.json` first and edit it in place. A full file:

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
  Text you leave out is kept readable for you; for a color you set, pick a
  darker background or a lighter text before giving up on an idea.
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

## 3. Tell the user

Say in one or two sentences what you made, and that they can go back with
`/wenlan theme reset` or with "Back to default" in the app's Appearance
settings.

`theme(action="reset")` goes back to the built-in look.
