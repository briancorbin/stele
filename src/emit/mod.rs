use crate::ir::Ir;

pub mod swift;
pub mod ts;

/// Every language backend implements this. Adding a language is one impl + one
/// line in `emitter_for` — the core IR never changes. This is the seam where
/// contributors plug in new targets.
pub trait Emitter {
    fn emit(&self, ir: &Ir) -> String;
}

pub fn emitter_for(lang: &str) -> Option<Box<dyn Emitter>> {
    match lang {
        "typescript" | "ts" => Some(Box::new(ts::TsEmitter)),
        "swift" => Some(Box::new(swift::SwiftEmitter)),
        _ => None,
    }
}
