//! An unescaped slot must not be renderable into an HTML template.
//!
//! `unescaped_text` writes its value verbatim, so allowing it inside HTML would
//! reintroduce the cross-site scripting hole the escaping slot exists to close.
//! The template's `Html` kind and the slot's `SlotFor<Plain>` impl make that a
//! compile error rather than a review comment.

use bytesbuf::mem::GlobalPool;
use routerama::response::bytesbuf::template::unescaped_text;
use routerama::response::html_view_template;

fn main() {
    let memory = GlobalPool::new();
    let page = html_view_template!(&memory, b"<p>", b"</p>");

    let _ = page.render(&memory, (unescaped_text("<script>alert('x')</script>"),));
}
