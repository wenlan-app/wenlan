// SPDX-License-Identifier: Apache-2.0
//! Immutable PDF location validation, shared by write and revision adoption.
use crate::{read_scope::ReadScope, WenlanError};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use wenlan_types::pages::PageCitation;

#[derive(Deserialize)]
struct History {
    current: Option<Document>,
    #[serde(default)]
    snapshots: Vec<Document>,
}
#[derive(Deserialize)]
struct Document {
    source_id: String,
    space: Option<String>,
    attachments: Vec<Attachment>,
}
#[derive(Deserialize)]
struct Attachment {
    key: String,
    cached_pdf_hash: Option<String>,
}

pub(crate) fn body_digest(body: &str) -> String {
    format!("{:x}", Sha256::digest(body.as_bytes()))
}

pub async fn validate_pdf_targets(
    citations: &[PageCitation],
    scope: &ReadScope,
) -> Result<(), WenlanError> {
    validate_pdf_targets_at(&crate::config::data_root().join("zotero"), citations, scope).await
}

async fn validate_pdf_targets_at(
    root: &std::path::Path,
    citations: &[PageCitation],
    scope: &ReadScope,
) -> Result<(), WenlanError> {
    let fail = || {
        WenlanError::Validation("invalid_pdf_target: source, attachment, digest, page or exact quote does not match preserved original PDF".into())
    };
    for citation in citations {
        let Some(target) = &citation.pdf_target else {
            continue;
        };
        if citation.source_kind != "memory"
            || target.page == 0
            || target.quote.trim().is_empty()
            || target.quote.len() > 16_000
            || target.pdf_digest.len() != 64
            || !target.pdf_digest.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(fail());
        }
        let source_hash = format!("{:x}", Sha256::digest(citation.locator.as_bytes()));
        let history: History = serde_json::from_slice(
            &tokio::fs::read(root.join("documents").join(format!("{source_hash}.json")))
                .await
                .map_err(|_| fail())?,
        )
        .map_err(|_| fail())?;
        // Authorization uses the live document; old snapshots only establish immutable attachment identity.
        let current = history.current.ok_or_else(fail)?;
        if current.source_id != citation.locator || !scope.matches(current.space.as_deref()) {
            return Err(fail());
        }
        let belongs = |d: &Document| {
            d.source_id == citation.locator
                && d.space == current.space
                && d.attachments.iter().any(|a| {
                    a.key == target.attachment_key
                        && a.cached_pdf_hash.as_deref() == Some(target.pdf_digest.as_str())
                })
        };
        if !belongs(&current) && !history.snapshots.iter().any(belongs) {
            return Err(fail());
        }
        let path = root.join("pdfs").join(format!("{}.pdf", target.pdf_digest));
        let meta = tokio::fs::symlink_metadata(&path)
            .await
            .map_err(|_| fail())?;
        if !meta.file_type().is_file() || meta.len() > 64 * 1024 * 1024 {
            return Err(fail());
        }
        let bytes = tokio::fs::read(path).await.map_err(|_| fail())?;
        if format!("{:x}", Sha256::digest(&bytes)) != target.pdf_digest
            || !bytes.starts_with(b"%PDF-")
        {
            return Err(fail());
        }
        let page = target.page;
        let (text, _) = tokio::task::spawn_blocking(move || {
            crate::sources::directory::extract_pdf_page(&bytes, page)
        })
        .await
        .map_err(|_| fail())?
        .map_err(|_| fail())?;
        let normalize = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
        if !normalize(&text).contains(&normalize(&target.quote)) {
            return Err(fail());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wenlan_types::pages::PdfCitationTarget;
    fn build_pdf(text: Option<&str>) -> Vec<u8> {
        use lopdf::content::{Content, Operation};
        use lopdf::{dictionary, Document, Object, Stream};

        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();

        let font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
        });
        let resources_id = doc.add_object(dictionary! {
            "Font" => dictionary! { "F1" => font_id },
        });

        let operations = match text {
            Some(t) => vec![
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec!["F1".into(), 24.into()]),
                Operation::new("Td", vec![20.into(), 100.into()]),
                Operation::new("Tj", vec![Object::string_literal(t)]),
                Operation::new("ET", vec![]),
            ],
            None => Vec::new(),
        };
        let content = Content { operations };
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));

        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
            "Resources" => resources_id,
            "MediaBox" => vec![0.into(), 0.into(), 300.into(), 144.into()],
        });

        let pages = dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page_id.into()],
            "Count" => 1,
        };
        doc.objects.insert(pages_id, Object::Dictionary(pages));

        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);

        let mut buf = Vec::new();
        doc.save_to(&mut buf).unwrap();
        buf
    }

    #[tokio::test]
    async fn immutable_pdf_target_checks_quote_page_bytes_scope_and_history() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        tokio::fs::create_dir(root.join("documents")).await.unwrap();
        tokio::fs::create_dir(root.join("pdfs")).await.unwrap();
        let bytes = build_pdf(Some(
            "Original measured evidence on the first physical page.",
        ));
        let hash = format!("{:x}", Sha256::digest(&bytes));
        tokio::fs::write(root.join("pdfs").join(format!("{hash}.pdf")), &bytes)
            .await
            .unwrap();
        let source = "zotero::paper";
        let source_hash = format!("{:x}", Sha256::digest(source.as_bytes()));
        let document = serde_json::json!({"source_id":source,"space":"Research","attachments":[{"key":"ATTACH01","cached_pdf_hash":hash}]});
        let path = root.join("documents").join(format!("{source_hash}.json"));
        tokio::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({"current":document})).unwrap(),
        )
        .await
        .unwrap();
        let citation = PageCitation {
            occurrence: 1,
            marker: 1,
            source_kind: "memory".into(),
            locator: source.into(),
            score: 0.0,
            status: "unevaluated".into(),
            scope: "sentence".into(),
            cited_version: None,
            cited_digest: None,
            disproven: false,
            historical: false,
            pdf_target: Some(PdfCitationTarget {
                attachment_key: "ATTACH01".into(),
                pdf_digest: hash.clone(),
                page: 1,
                quote: "measured evidence\non the first physical page.".into(),
            }),
        };
        let scope = ReadScope::Space("Research".into());
        validate_pdf_targets_at(root, std::slice::from_ref(&citation), &scope)
            .await
            .unwrap();
        for (page, quote, attachment) in [
            (2, "measured evidence", "ATTACH01"),
            (1, "fabricated evidence", "ATTACH01"),
            (1, "measured evidence", "FOREIGN1"),
        ] {
            let mut bad = citation.clone();
            let target = bad.pdf_target.as_mut().unwrap();
            target.page = page;
            target.quote = quote.into();
            target.attachment_key = attachment.into();
            assert!(validate_pdf_targets_at(root, &[bad], &scope).await.is_err());
        }
        assert!(validate_pdf_targets_at(
            root,
            std::slice::from_ref(&citation),
            &ReadScope::Space("Other".into())
        )
        .await
        .is_err());
        // A replaced attachment may still be cited as its explicit preserved historical bytes.
        let current = serde_json::json!({"source_id":source,"space":"Research","attachments":[]});
        tokio::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({"current":current,"snapshots":[document]}))
                .unwrap(),
        )
        .await
        .unwrap();
        validate_pdf_targets_at(root, std::slice::from_ref(&citation), &scope)
            .await
            .unwrap();
        tokio::fs::write(
            root.join("pdfs").join(format!("{hash}.pdf")),
            b"%PDF-corrupted",
        )
        .await
        .unwrap();
        assert!(validate_pdf_targets_at(root, &[citation], &scope)
            .await
            .is_err());
    }
}

/// Current managed attachment metadata only; historical snapshots never establish
/// fresh evidence for a new human confirmation. No PDF parsing occurs here.
pub async fn current_pdf_digest(
    source_id: &str,
    attachment_key: &str,
    scope: &ReadScope,
) -> Result<Option<String>, WenlanError> {
    current_pdf_digest_at(
        &crate::config::data_root().join("zotero"),
        source_id,
        attachment_key,
        scope,
    )
    .await
}
async fn current_pdf_digest_at(
    root: &std::path::Path,
    source_id: &str,
    attachment_key: &str,
    scope: &ReadScope,
) -> Result<Option<String>, WenlanError> {
    let hash = body_digest(source_id);
    let path = root.join("documents").join(format!("{hash}.json"));
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(_) => return Ok(None),
    };
    let history: History = match serde_json::from_slice(&bytes) {
        Ok(history) => history,
        Err(_) => return Ok(None),
    };
    let Some(current) = history.current else {
        return Ok(None);
    };
    if current.source_id != source_id || !scope.matches(current.space.as_deref()) {
        return Ok(None);
    }
    Ok(current
        .attachments
        .into_iter()
        .find(|a| a.key == attachment_key)
        .and_then(|a| a.cached_pdf_hash))
}

#[cfg(test)]
mod current_digest_tests {
    use super::*;
    #[tokio::test]
    async fn dispute_fence_uses_current_attachment_identity_not_retained_history() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        tokio::fs::create_dir(root.join("documents")).await.unwrap();
        let source = "zotero::same-attachment";
        let path = root
            .join("documents")
            .join(format!("{}.json", body_digest(source)));
        let current = serde_json::json!({"source_id":source,"space":"Research","attachments":[{"key":"ATTACH01","cached_pdf_hash":"new-digest"}]});
        let prior = serde_json::json!({"source_id":source,"space":"Research","attachments":[{"key":"ATTACH01","cached_pdf_hash":"old-digest"}]});
        tokio::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({"current":current,"snapshots":[prior]}))
                .unwrap(),
        )
        .await
        .unwrap();
        let scope = ReadScope::Space("Research".into());
        assert_eq!(
            current_pdf_digest_at(root, source, "ATTACH01", &scope)
                .await
                .unwrap()
                .as_deref(),
            Some("new-digest")
        );
        assert!(
            current_pdf_digest_at(root, source, "ATTACH01", &ReadScope::Space("Other".into()))
                .await
                .unwrap()
                .is_none()
        );
        assert!(current_pdf_digest_at(root, source, "missing", &scope)
            .await
            .unwrap()
            .is_none());
        tokio::fs::write(&path, b"broken sidecar").await.unwrap();
        assert!(current_pdf_digest_at(root, source, "ATTACH01", &scope)
            .await
            .unwrap()
            .is_none());
    }
}
