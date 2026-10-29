// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useId, useRef, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import "./source-access.css";

export default function SourceDialog({title,lead,onClose,children}:{title:string;lead?:string;onClose:()=>void;children:ReactNode}) {
  const {t}=useTranslation();
  const titleId=useId();
  const ref=useRef<HTMLDivElement>(null);
  const close=useRef(onClose);close.current=onClose;
  useEffect(()=>{
    const trigger=document.activeElement instanceof HTMLElement?document.activeElement:null;
    const controls=()=>Array.from(ref.current?.querySelectorAll<HTMLElement>('button:not(:disabled),input:not(:disabled),textarea:not(:disabled),select:not(:disabled),[tabindex="0"]')??[])
      .sort((left,right)=>left.compareDocumentPosition(right)&Node.DOCUMENT_POSITION_FOLLOWING?-1:1);
    controls()[0]?.focus();
    const key=(e:KeyboardEvent)=>{
      if(e.key==='Escape'&&!e.isComposing){e.preventDefault();e.stopPropagation();close.current();}
      if(e.key==='Tab'){
        const items=controls();const first=items[0],last=items[items.length-1];
        if(e.shiftKey&&document.activeElement===first){e.preventDefault();last?.focus();}
        else if(!e.shiftKey&&document.activeElement===last){e.preventDefault();first?.focus();}
      }
    };
    const focus=(e:FocusEvent)=>{if(e.target instanceof Node&&!ref.current?.contains(e.target))controls()[0]?.focus();};
    document.addEventListener('keydown',key,true);document.addEventListener('focusin',focus);
    return()=>{document.removeEventListener('keydown',key,true);document.removeEventListener('focusin',focus);if(trigger?.isConnected)trigger.focus();};
  },[]);
  return createPortal(<div className="source-dialog-overlay" onClick={e=>{if(e.target===e.currentTarget)onClose();}}>
    <div role="dialog" aria-modal="true" aria-labelledby={titleId} className="source-dialog" ref={ref}>
      <header><div className="source-dialog-heading"><h2 id={titleId}>{title}</h2>{lead?<p className="source-dialog-lead">{lead}</p>:null}</div><button type="button" className="mem-icon-action" aria-label={t('sourceAccess.close')} onClick={onClose}><svg aria-hidden="true" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5"><path d="m6 6 12 12M6 18 18 6"/></svg></button></header>
      {children}
    </div>
  </div>,document.body);
}
