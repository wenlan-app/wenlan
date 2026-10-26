// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ReferencePreview } from "./ReferencePreview";
import { useReferenceNavigation } from "./ReferenceNavigationContext";
import { referenceTargetFromHref } from "./referenceTypes";
import { ReferenceTypeIcon } from "./ReferenceTypeIcon";
import "./reference-links.css";

interface ReferenceLinkProps extends React.AnchorHTMLAttributes<HTMLAnchorElement> {
  href: string;
  children: React.ReactNode;
}

export function ReferenceLink({ href, children, className, onClick, ...props }: ReferenceLinkProps) {
  const target = referenceTargetFromHref(href);
  if (!target) {
    return <a href={href} className={className} onClick={onClick} {...props}>{children}</a>;
  }
  return <CanonicalReferenceLink href={href} referenceTarget={target} className={className} onClick={onClick} {...props}>{children}</CanonicalReferenceLink>;
}

function CanonicalReferenceLink({
  href, referenceTarget, children, className, onClick, "aria-label": ariaLabel, ...props
}: ReferenceLinkProps & { referenceTarget: NonNullable<ReturnType<typeof referenceTargetFromHref>> }) {
  const { t } = useTranslation();
  const navigation = useReferenceNavigation();
  const [preview, setPreview] = useState<null | { anchor: HTMLAnchorElement; keyboard: boolean }>(null);
  const closeTimer = useRef<number | null>(null);
  const touchTap = useRef(false);
  const suppressFocusPreview = useRef(false);
  const label = typeof children === "string" ? children : undefined;
  const kind = referenceTarget.kind;
  const typeLabel = t(`reference.type.${kind}`);

  const clearTimers = () => {
    if (closeTimer.current !== null) window.clearTimeout(closeTimer.current);
    closeTimer.current = null;
  };
  useEffect(() => clearTimers, []);
  const schedulePreview = (anchor: HTMLAnchorElement, keyboard: boolean) => {
    if (closeTimer.current !== null) window.clearTimeout(closeTimer.current);
    closeTimer.current = null;
    setPreview({ anchor, keyboard });
  };
  const closePreview = () => {
    if (closeTimer.current !== null) window.clearTimeout(closeTimer.current);
    closeTimer.current = window.setTimeout(() => setPreview(null), 120);
  };
  const openTarget = () => {
    if (referenceTarget.kind === "page") navigation.onOpenPage?.(referenceTarget.id);
    else navigation.onOpenMemory?.(referenceTarget.id);
  };

  return (
    <>
      <a
        {...props}
        href={href}
        className={["reference-link", `reference-link--${kind}`, className].filter(Boolean).join(" ")}
        data-reference-kind={kind}
        data-reference-id={referenceTarget.id}
        data-reference-navigation="true"
        data-reference-link="true"
        aria-label={ariaLabel}
        title={label ? `${typeLabel}: ${label}` : typeLabel}
        style={props.style}
        onPointerDown={(event) => {
          touchTap.current = event.pointerType === "touch";
          props.onPointerDown?.(event);
        }}
        onPointerEnter={(event) => {
          schedulePreview(event.currentTarget, false);
          props.onPointerEnter?.(event);
        }}
        onPointerLeave={(event) => {
          closePreview();
          props.onPointerLeave?.(event);
        }}
        onFocus={(event) => {
          if (suppressFocusPreview.current) suppressFocusPreview.current = false;
          else schedulePreview(event.currentTarget, true);
          props.onFocus?.(event);
        }}
        onBlur={(event) => {
          closePreview();
          props.onBlur?.(event);
        }}
        onClick={(event) => {
          onClick?.(event);
          if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
          event.preventDefault();
          if (touchTap.current) {
            // WebKit emits pointerleave before its compatibility click.
            schedulePreview(event.currentTarget, false);
            touchTap.current = false;
            return;
          }
          openTarget();
        }}
      >
        <ReferenceTypeIcon kind={kind} />
        <span>{children}</span>
      </a>
      {preview && <ReferencePreview
        request={{ target: referenceTarget, anchor: preview.anchor, keyboard: preview.keyboard }}
        onDismiss={() => setPreview(null)}
        onOpenPage={(id) => navigation.onOpenPage?.(id)}
        onOpenMemory={(id) => navigation.onOpenMemory?.(id)}
        onEscape={() => {
          clearTimers();
          suppressFocusPreview.current = true;
          setPreview(null);
          preview.anchor.focus({ preventScroll: true });
        }}
        onPointerEnter={() => {
          if (closeTimer.current !== null) window.clearTimeout(closeTimer.current);
          closeTimer.current = null;
        }}
        onPointerLeave={closePreview}
      />}
    </>
  );
}
