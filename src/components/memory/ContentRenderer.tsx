// SPDX-License-Identifier: AGPL-3.0-only
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import {
  classifyContent,
  prepareForRender,
  extractPreview,
  normalizeContent,
} from "../../lib/contentClassifier";
import { CITATION_ANCHOR_PREFIX } from "../../lib/pageCitations";
import { ReferenceLink } from "./links/ReferenceLink";
import { referenceTargetFromHref } from "./links/referenceTypes";
import { displayUrl, isBareUrlLink } from "../../lib/displayUrl";
import MermaidDiagram from "./MermaidDiagram";
import { attachmentKind, attachmentName, attachmentUrl, useAttachmentsDir, videoSource } from "../../lib/pageAttachments";

interface ContentRendererProps {
  content: string;
  structuredFields?: string | null;
  /** "lede" renders bare paragraphs so the surrounding pull-quote styles apply. */
  variant: "card" | "detail" | "lede";
  className?: string;
  /** Render #citation:k links as inline chips (page detail). */
  renderCitation?: (occurrence: number) => React.ReactNode;
  /** Imported source previews must not load remote images while being read. */
  allowImages?: boolean;
}

interface HastNode {
  type?: string;
  tagName?: string;
  value?: string;
  properties?: { className?: unknown };
  children?: HastNode[];
}

/** The source of a fenced ```mermaid block, or null for any other <pre>. */
function mermaidSource(node: HastNode | undefined): string | null {
  const code = node?.children?.find((child) => child.type === "element");
  const classes = code?.tagName === "code" ? code.properties?.className : undefined;
  if (!Array.isArray(classes) || !classes.includes("language-mermaid")) return null;
  const text = (code?.children ?? []).map((child) => child.value ?? "").join("");
  return text.trim() ? text : null;
}

/** An image or video a page shows from the pages folder's `_attachments/`;
 * any other image link renders as before. */
function MarkdownImage({ src, alt }: { src?: string; alt?: string }) {
  const name = attachmentName(src);
  if (!name) return <img src={src} alt={alt ?? ""} />;
  return <AttachmentMedia src={src!} name={name} alt={alt ?? ""} />;
}

function AttachmentMedia({ src, name, alt }: { src: string; name: string; alt: string }) {
  const url = attachmentUrl(src, useAttachmentsDir());
  if (!url) return <span className="page-attachment-pending">{alt}</span>;
  return attachmentKind(name) === "video" ? (
    <video className="page-attachment page-attachment-video" src={videoSource(url)} controls preload="metadata" aria-label={alt || name} />
  ) : (
    <img className="page-attachment" src={url} alt={alt} loading="lazy" decoding="async" />
  );
}

export const markdownComponents = {
  img: ({ src, alt }: { src?: string; alt?: string }) => <MarkdownImage src={src} alt={alt} />,
  pre: ({ node, children }: { node?: HastNode; children?: React.ReactNode }) => {
    const source = mermaidSource(node);
    return source !== null ? <MermaidDiagram source={source} /> : <pre>{children}</pre>;
  },
  p: ({ children }: { children?: React.ReactNode }) => (
    <p
      style={{
        fontFamily: "var(--mem-font-body)",
        fontSize: "var(--content-body-size, 14px)",
        lineHeight: "var(--content-body-leading, 1.7)",
        color: "var(--mem-text)",
        marginBottom: "var(--content-para-gap, 0.75em)",
      }}
    >
      {children}
    </p>
  ),
  strong: ({ children }: { children?: React.ReactNode }) => (
    <strong
      style={{
        color: "var(--mem-text)",
        fontWeight: 600,
      }}
    >
      {children}
    </strong>
  ),
  li: ({ children }: { children?: React.ReactNode }) => (
    <li
      style={{
        fontFamily: "var(--mem-font-body)",
        fontSize: "var(--content-body-size, 14px)",
        lineHeight: "var(--content-body-leading, 1.7)",
        color: "var(--mem-text)",
        paddingLeft: "0.25em",
      }}
    >
      {children}
    </li>
  ),
  ul: ({ children }: { children?: React.ReactNode }) => (
    <ul
      style={{
        listStyleType: "disc",
        paddingLeft: "1.5em",
        color: "var(--mem-text-tertiary)",
        marginBottom: "var(--content-para-gap, 0.75em)",
      }}
    >
      {children}
    </ul>
  ),
  ol: ({ children }: { children?: React.ReactNode }) => (
    <ol
      style={{
        listStyleType: "decimal",
        paddingLeft: "1.5em",
        color: "var(--mem-text-tertiary)",
        marginBottom: "var(--content-para-gap, 0.75em)",
      }}
    >
      {children}
    </ol>
  ),
  a: ({ href, children }: { href?: string; children?: React.ReactNode }) => (
    referenceTargetFromHref(href)
      ? <ReferenceLink href={href!}>{children}</ReferenceLink>
      : <a
          href={href}
          target={href?.startsWith("#") ? undefined : "_blank"}
          rel="noopener noreferrer"
          title={isBareUrlLink(href, children) ? href : undefined}
        >
          {isBareUrlLink(href, children) ? displayUrl(children) : children}
        </a>
  ),
  h1: ({ children }: { children?: React.ReactNode }) => (
    <h1
      style={{
        fontFamily: "var(--mem-font-heading)",
        fontSize: "var(--content-h1-size, 18px)",
        fontWeight: 500,
        color: "var(--mem-text)",
        marginTop: "1em",
        marginBottom: "0.5em",
      }}
    >
      {children}
    </h1>
  ),
  h2: ({ children }: { children?: React.ReactNode }) => (
    <h2
      style={{
        fontFamily: "var(--mem-font-heading)",
        fontSize: "var(--content-h2-size, 16px)",
        fontWeight: 500,
        color: "var(--mem-text)",
        marginTop: "var(--content-heading-gap, 1em)",
        marginBottom: "0.5em",
      }}
    >
      {children}
    </h2>
  ),
  h3: ({ children }: { children?: React.ReactNode }) => (
    <h3
      style={{
        fontFamily: "var(--mem-font-heading)",
        fontSize: "var(--content-h3-size, 14px)",
        fontWeight: 600,
        color: "var(--mem-text)",
        marginTop: "0.75em",
        marginBottom: "0.25em",
      }}
    >
      {children}
    </h3>
  ),
  code: ({
    children,
    className,
  }: {
    children?: React.ReactNode;
    className?: string;
  }) => {
    if (!className) {
      return (
        <code
          style={{
            fontFamily: "var(--mem-font-mono)",
            fontSize: "var(--content-code-size, 12px)",
            backgroundColor: "var(--mem-hover)",
            padding: "0.15em 0.4em",
            borderRadius: "3px",
          }}
        >
          {children}
        </code>
      );
    }
    return (
      <code
        className={className}
        style={{
          fontFamily: "var(--mem-font-mono)",
          fontSize: "var(--content-code-size, 12px)",
        }}
      >
        {children}
      </code>
    );
  },
};

function CardPreview({
  content,
  structuredFields,
}: {
  content: string;
  structuredFields?: string | null;
}) {
  if (!content.trim()) {
    return (
      <span style={{ color: "var(--mem-text-tertiary)", fontStyle: "italic" }}>
        No content
      </span>
    );
  }

  const normalized = normalizeContent(content);
  const shape = classifyContent(normalized, structuredFields);
  const preview = extractPreview(normalized, shape);

  if (preview === null) {
    return <>{content.split("\n")[0]}</>;
  }

  if (typeof preview === "object") {
    return (
      <>
        <strong style={{ fontWeight: 600 }}>{preview.key}:</strong>{" "}
        {preview.value}
      </>
    );
  }

  // For prose-like single-facts with multiple sentences, show only the first sentence
  if (shape === "single-fact" && preview.includes(". ")) {
    const sentenceEnd = preview.indexOf(". ");
    return <>{preview.slice(0, sentenceEnd + 1)}</>;
  }

  return <>{preview}</>;
}

export default function ContentRenderer({
  content,
  structuredFields,
  variant,
  className,
  renderCitation,
  allowImages = true,
}: ContentRendererProps) {
  if (variant === "card") {
    return (
      <span className={className}>
        <CardPreview content={content} structuredFields={structuredFields} />
      </span>
    );
  }

  if (!content.trim()) {
    return (
      <p
        style={{
          color: "var(--mem-text-tertiary)",
          fontStyle: "italic",
          fontFamily: "var(--mem-font-body)",
          fontSize: "14px",
        }}
      >
        No content
      </p>
    );
  }

  const normalized = normalizeContent(content);
  const shape = classifyContent(normalized, structuredFields);
  const prepared = prepareForRender(normalized, shape);

  const baseComponents =
    variant === "lede"
      ? {
          ...markdownComponents,
          p: ({ children }: { children?: React.ReactNode }) => <p>{children}</p>,
        }
      : markdownComponents;
  const components = renderCitation
    ? {
        ...baseComponents,
        a: (props: { href?: string; children?: React.ReactNode }) => {
          const href = props.href ?? "";
          if (href.startsWith(CITATION_ANCHOR_PREFIX)) {
            const k = Number(href.slice(CITATION_ANCHOR_PREFIX.length));
            if (Number.isInteger(k) && k > 0) return <>{renderCitation(k)}</>;
          }
          return markdownComponents.a(props);
        },
      }
    : baseComponents;

  return (
    <div className={["content-renderer", className].filter(Boolean).join(" ")}>
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={allowImages ? components : {
        ...components,
        img: ({ alt }: { alt?: string }) => <span>{alt}</span>,
      }}>
        {prepared}
      </ReactMarkdown>
    </div>
  );
}
