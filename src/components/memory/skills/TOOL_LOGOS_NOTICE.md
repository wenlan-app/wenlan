# Tool logos

`toolLogos.ts` holds monochrome SVG path data for the AI tools the rules page
names. The paths are copied unchanged from two openly licensed icon sets; no
icon package is a dependency and nothing is fetched at run time. They are drawn
in each brand's own colour where that colour has at least 3:1 contrast with
the chip in the theme shown (#FFFFFF light, #202430 dark), and in the theme's
text colour otherwise.

The names and logos are trademarks of their owners. They are shown only to say
which tool a chip stands for.

| Tool id | Source | Icon | License | Brand colour | Light | Dark |
|---|---|---|---|---|---|---|
| `claude-code` | simple-icons 16.38.0 | `claudecode` | CC0-1.0 | #D97757 | brand | brand |
| `claude-desktop` | simple-icons 16.38.0 | `claude` | CC0-1.0 | #D97757 | brand | brand |
| `cursor` | simple-icons 16.38.0 | `cursor` | CC0-1.0 | #000000 | text | text |
| `windsurf` | simple-icons 16.38.0 | `windsurf` | CC0-1.0 | #0B100F | text | text |
| `copilot-cli` | simple-icons 16.38.0 | `githubcopilot` | MIT (Primer Octicons, © GitHub Inc.) | #000000 | text | text |
| `zed` | simple-icons 16.38.0 | `zedindustries` | CC0-1.0 | #084CCF | brand | text |
| `opencode` | simple-icons 16.38.0 | `opencode` | CC0-1.0 | #000000 | text | text |
| `kimi-code`, `kimi-desktop` | simple-icons 16.38.0 | `kimi` | CC0-1.0 | #000000 | text | text |
| `raycast` | simple-icons 16.38.0 | `raycast` | CC0-1.0 | #FF6363 | text | brand |
| `obsidian` | simple-icons 16.38.0 | `obsidian` | CC0-1.0 | #7C3AED | brand | text |
| `logseq` | simple-icons 16.38.0 | `logseq` | CC0-1.0 | #85C8C8 | text | brand |
| `codex` | @lobehub/icons-static-svg 1.103.0 | `codex` | MIT | #000000 (OpenAI) | text | text |
| `kiro` | @lobehub/icons-static-svg 1.103.0 | `kiro` | MIT | #9046FF | brand | brand |
| `antigravity` | @lobehub/icons-static-svg 1.103.0 | `antigravity` | MIT | #3186FF | brand | brand |
| `gemini-cli` | @lobehub/icons-static-svg 1.103.0 | `geminicli` | MIT | #207CFE | brand | brand |
| `chatgpt`, `openai-mcp` | @lobehub/icons-static-svg 1.103.0 | `openai` | MIT | #000000 | text | text |

Tools with no logo here (`vscode`, `continue`, `muse-code`, and any tool the
daemon names that is not listed) show a neutral initial badge instead.

Brand colours: simple-icons' `hex` for each icon (data/simple-icons.json in
16.38.0), which names the brand's own page as its source. For the lobehub
icons, the colour variant in @lobehub/icons-static-svg 1.103.0
(`kiro-color.svg` #9046FF; `antigravity-color.svg`, several colours, its blue
#3186FF; `geminicli-color.svg`, a gradient, its first stop #207CFE). Codex and
OpenAI are drawn as OpenAI's black, in the text colour; lobehub's Codex
colour variant is a blue gradient that was not used.

- simple-icons: https://github.com/simple-icons/simple-icons, CC0 1.0 Universal,
  except GitHub Copilot (below).
- GitHub Copilot (`githubcopilot` in simple-icons, from Primer Octicons,
  https://primer.style/foundations/icons/copilot-24): MIT License,
  Copyright (c) GitHub Inc.

  Permission is hereby granted, free of charge, to any person obtaining a copy
  of this software and associated documentation files (the "Software"), to deal
  in the Software without restriction, including without limitation the rights
  to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
  copies of the Software, and to permit persons to whom the Software is
  furnished to do so, subject to the following conditions:

  The above copyright notice and this permission notice shall be included in
  all copies or substantial portions of the Software.

  THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
  IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
  FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
  AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
  LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
  OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
  SOFTWARE.
- @lobehub/icons-static-svg: https://github.com/lobehub/lobe-icons, MIT License,
  Copyright (c) LobeHub.

  Permission is hereby granted, free of charge, to any person obtaining a copy
  of this software and associated documentation files (the "Software"), to deal
  in the Software without restriction, including without limitation the rights
  to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
  copies of the Software, and to permit persons to whom the Software is
  furnished to do so, subject to the following conditions:

  The above copyright notice and this permission notice shall be included in
  all copies or substantial portions of the Software.

  THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
  IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
  FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
  AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
  LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
  OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
  SOFTWARE.
