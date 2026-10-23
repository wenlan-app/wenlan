// SPDX-License-Identifier: AGPL-3.0-only
import { useMemo } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { markdownComponents } from "./ContentRenderer";
import { markdownDiff, rehypeDiffMarks } from "../../lib/markdownDiff";
import "./markdownDiff.css";

interface MarkdownDiffProps {
  before: string;
  after: string;
  /** Both sides marked in one text, or only the side named. */
  side: "unified" | "before" | "after";
  insStyle: React.CSSProperties;
  delStyle: React.CSSProperties;
}

type MarkProps = React.HTMLAttributes<HTMLElement> & { node?: unknown; "data-diff"?: string };

/** A revision rendered the way the page reads, with the changed words marked:
 * headings, lists and emphasis show formatted instead of as `#` and `**`. */
export default function MarkdownDiff({ before, after, side, insStyle, delStyle }: MarkdownDiffProps) {
  const markdown = useMemo(() => markdownDiff(before, after, side), [before, after, side]);
  const components = useMemo(
    () => ({
      ...markdownComponents,
      // A strikethrough the page itself has stays a plain <del>.
      ins: ({ node: _node, ...props }: MarkProps) =>
        props["data-diff"] ? <ins {...props} style={insStyle} /> : <ins {...props} />,
      del: ({ node: _node, ...props }: MarkProps) =>
        props["data-diff"] ? <del {...props} style={delStyle} /> : <del {...props} />,
    }),
    [insStyle, delStyle],
  );
  return (
    <div className="content-renderer markdown-diff">
      <ReactMarkdown remarkPlugins={[remarkGfm]} rehypePlugins={[rehypeDiffMarks]} components={components}>
        {markdown}
      </ReactMarkdown>
    </div>
  );
}
