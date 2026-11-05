//! Laying the pages of one PDF over the pages of another.

use lopdf::{Dictionary, Document, Object, ObjectId, Stream, dictionary};

use super::{add_page_resource, get_i64, isolate_page_content, malformed};
use crate::error::Result;

/// What an overlay page brings along once its objects live in the base.
struct Imported {
    id: ObjectId,
    content: Vec<u8>,
    resources: Object,
    media_box: Object,
}

/// Imports every page of `over` into `base`: as a form XObject drawn over
/// the matching base page, or as a whole page appended after the last one.
pub(super) fn overlay(mut base: Document, mut over: Document) -> Result<Document> {
    let base_max = base
        .objects
        .keys()
        .map(|id| id.0)
        .max()
        .unwrap_or(0)
        .max(base.max_id);
    over.renumber_objects_with(base_max + 1);

    let imports = over
        .page_iter()
        .map(|id| {
            let page = over.get_dictionary(id).map_err(malformed)?;
            Ok(Imported {
                id,
                content: over.get_page_content(id),
                resources: inherited(&over, page, b"Resources")
                    .unwrap_or_else(|| Dictionary::new().into()),
                media_box: inherited(&over, page, b"MediaBox")
                    .unwrap_or_else(|| vec![0.into(), 0.into(), 612.into(), 792.into()].into()),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let base_pages: Vec<ObjectId> = base.page_iter().collect();

    let over_max = over.max_id;
    base.objects.append(&mut over.objects);
    base.max_id = base.max_id.max(over_max);

    for (i, imported) in imports.into_iter().enumerate() {
        match base_pages.get(i) {
            Some(&page_id) => draw_over(&mut base, page_id, i + 1, imported)?,
            None => append_page(&mut base, imported)?,
        }
    }
    base.prune_objects();
    Ok(base)
}

/// Turns the imported page into a form XObject and draws it on `page_id`.
fn draw_over(base: &mut Document, page_id: ObjectId, n: usize, imported: Imported) -> Result<()> {
    let xobject = Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "BBox" => imported.media_box,
            "Matrix" => vec![1.into(), 0.into(), 0.into(), 1.into(), 0.into(), 0.into()],
            "Resources" => imported.resources,
        },
        imported.content,
    );
    let xid = base.add_object(xobject);
    let name = format!("RdOv{n}");
    add_page_resource(base, page_id, b"XObject", &name, xid)?;
    isolate_page_content(base, page_id)?;
    base.add_page_contents(page_id, format!("\nq /{name} Do Q\n").into_bytes())
        .map_err(malformed)
}

/// Hangs the imported page, with its inherited attributes made explicit,
/// under the base page tree root.
fn append_page(base: &mut Document, imported: Imported) -> Result<()> {
    let root_id = base
        .catalog()
        .and_then(|c| c.get(b"Pages"))
        .and_then(Object::as_reference)
        .map_err(malformed)?;
    let page = base.get_dictionary_mut(imported.id).map_err(malformed)?;
    page.set("Parent", Object::Reference(root_id));
    page.set("Resources", imported.resources);
    page.set("MediaBox", imported.media_box);
    let root = base.get_dictionary(root_id).map_err(malformed)?;
    let mut kids = super::get(base, root, b"Kids")
        .and_then(|k| k.as_array().ok())
        .cloned()
        .unwrap_or_default();
    kids.push(Object::Reference(imported.id));
    let count = get_i64(base, root, b"Count").unwrap_or(kids.len() as i64 - 1) + 1;
    let root = base.get_dictionary_mut(root_id).map_err(malformed)?;
    root.set("Kids", kids);
    root.set("Count", count);
    Ok(())
}

/// A page attribute, looked up through the page tree when the page itself
/// lacks it. References are kept as they are.
fn inherited(doc: &Document, page: &Dictionary, key: &[u8]) -> Option<Object> {
    let mut node = page;
    for _ in 0..64 {
        if let Ok(value) = node.get(key) {
            return Some(value.clone());
        }
        let parent = node.get(b"Parent").ok()?.as_reference().ok()?;
        node = doc.get_dictionary(parent).ok()?;
    }
    None
}
