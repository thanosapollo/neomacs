use super::{Face, FaceTable, Value, ValueKind, face_symbol_value, normalized_face_name_value};
use crate::emacs_core::value::list_to_vec;
use std::collections::{HashMap, HashSet};

// ---------------------------------------------------------------------------
// Face remapping (face-remapping-alist support)
// ---------------------------------------------------------------------------

/// A single entry in a remapping specification.
///
/// Corresponds to the CDR of an entry in `face-remapping-alist`:
/// - `(FACE . other-face)`        -> `[RemapFace("other-face")]`
/// - `(FACE . (:attr val ...))`   -> `[RemapAttrs(face)]`
/// - `(FACE . (a b (:k v) ...))`  -> mixed list of face names & attr plists
#[derive(Clone, Debug)]
pub enum FaceRemapEntry {
    /// Remap to another named face.
    RemapFace(Value),
    /// Inline attributes and their original order (GNU merges plists in order).
    RemapAttrs(Face, Vec<Value>),
}

/// Parsed form of the buffer-local `face-remapping-alist`.
///
/// Maps original face name -> ordered list of remapping entries.
/// When resolving face `X`, if `X` is in this map the entries replace the
/// original face definition.
#[derive(Clone, Debug, Default)]
pub struct FaceRemapping {
    map: HashMap<Value, Vec<FaceRemapEntry>>,
}

impl FaceRemapping {
    /// Create an empty (no remapping) instance.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether there are any remappings.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Insert a remapping for the given face name.
    pub fn insert(&mut self, face_name: Value, entries: Vec<FaceRemapEntry>) {
        self.map.insert(face_name, entries);
    }

    /// Look up the remapping entries for a face name.
    pub fn get(&self, face_name: &str) -> Option<&[FaceRemapEntry]> {
        self.map
            .get(&face_symbol_value(face_name))
            .map(|v| v.as_slice())
    }

    /// Parse `face-remapping-alist` from its Lisp value.
    ///
    /// The alist has the form `((FACE . SPEC) ...)` where SPEC can be:
    /// - A symbol (face name)
    /// - A plist `(:attr val ...)`
    /// - A list of specs `(face1 face2 (:attr val ...) ...)`
    pub fn from_lisp(value: &Value) -> Self {
        use crate::emacs_core::value::list_to_vec;

        let mut remapping = Self::new();

        let Some(alist) = list_to_vec(value) else {
            return remapping;
        };

        for entry in &alist {
            // Each entry is (FACE . SPEC) — a cons cell
            if !entry.is_cons() {
                continue;
            };
            let cell_car = entry.cons_car();
            let cell_cdr = entry.cons_cdr();
            let Some(face_name) = normalized_face_name_value(&cell_car) else {
                continue;
            };
            if face_name.is_symbol_named("nil") {
                continue;
            }

            let entries = Self::parse_remap_spec(&cell_cdr);
            if !entries.is_empty() {
                remapping.insert(face_name, entries);
            }
        }

        remapping
    }

    /// Parse a single remapping spec (the CDR of an alist entry).
    fn parse_remap_spec(spec: &Value) -> Vec<FaceRemapEntry> {
        use crate::emacs_core::value::list_to_vec;

        match spec.kind() {
            // Simple symbol remap: (FACE . other-face)
            ValueKind::Symbol(_) | ValueKind::T | ValueKind::String => {
                if let Some(name) = normalized_face_name_value(spec)
                    && !name.is_symbol_named("nil")
                {
                    return vec![FaceRemapEntry::RemapFace(name)];
                }
                Vec::new()
            }
            ValueKind::Nil => Vec::new(),
            // List form: could be a plist or a list of specs
            ValueKind::Cons => {
                let Some(items) = list_to_vec(spec) else {
                    return Vec::new();
                };
                if items.is_empty() {
                    return Vec::new();
                }

                // Check if it's a plist (starts with keyword)
                if items[0].as_keyword_id().is_some() {
                    let face = Face::from_plist("--remap--", &items);
                    return vec![FaceRemapEntry::RemapAttrs(face, items)];
                }

                // Otherwise it's a list of specs: (face1 face2 (:k v ...) ...)
                let mut entries = Vec::new();
                for item in &items {
                    match item.kind() {
                        ValueKind::Symbol(_) | ValueKind::T | ValueKind::String => {
                            if let Some(name) = normalized_face_name_value(item)
                                && !name.is_symbol_named("nil")
                            {
                                entries.push(FaceRemapEntry::RemapFace(name));
                            }
                        }
                        ValueKind::Cons => {
                            if let Some(sub_items) = list_to_vec(item)
                                && sub_items.first().is_some_and(|v| v.is_keyword())
                            {
                                let face = Face::from_plist("--remap--", &sub_items);
                                entries.push(FaceRemapEntry::RemapAttrs(face, sub_items));
                            }
                        }
                        _ => {}
                    }
                }
                entries
            }
            _ => Vec::new(),
        }
    }
}

/// Named queries resolve inheritance and start with the default face.
/// Text-face layers contribute only specified attributes to their caller's
/// existing base. Both modes share remapping precedence and recursion.
#[derive(Clone, Copy)]
enum RemappingBase {
    DefaultAndInherited,
    SpecifiedOnly,
    InheritedOnly,
}

impl FaceTable {
    /// GNU lookup_named_face starts with canonical DEFAULT_FACE_ID attributes,
    /// then merges the (possibly remapped) named reference into that base.
    /// Unlike text realization, an implicit default is not buffer-remapped.
    /// Merge inheritance into the existing base before direct attributes so
    /// relative heights are applied, and rounded, at each GNU merge boundary.
    pub fn resolve_for_font(&self, name: &str, remapping: &FaceRemapping) -> Face {
        let mut result = self.resolve("default");
        self.merge_font_reference(
            &mut result,
            face_symbol_value(name),
            remapping,
            &mut Vec::new(),
            0,
        );
        result.inherit = None;
        result
    }

    fn merge_font_reference(
        &self,
        base: &mut Face,
        reference: Value,
        remapping: &FaceRemapping,
        seen: &mut Vec<(Value, bool)>,
        depth: usize,
    ) {
        if depth > 40 || reference.is_nil() {
            return;
        }
        if let Some(reference) = normalized_face_name_value(&reference) {
            let name = reference.as_symbol_name().unwrap();
            // lookup_named_face's initial vector lookup has no normal merge
            // point; merge_named_face installs one before looking up remaps.
            let normal = depth != 0;
            if normal && !Self::push_font_merge_point(seen, reference, false) {
                return;
            }
            if let Some(entries) = remapping.get(name)
                && Self::push_font_merge_point(seen, reference, true)
            {
                // get_lface_attributes builds a remapping in an unspecified
                // vector first; relative entries compose before meeting base.
                let mut contribution = Face::new(name);
                for entry in entries.iter().rev() {
                    match entry {
                        FaceRemapEntry::RemapFace(target) => {
                            self.merge_font_reference(
                                &mut contribution,
                                *target,
                                remapping,
                                seen,
                                depth + 1,
                            );
                        }
                        FaceRemapEntry::RemapAttrs(_, items) => {
                            self.merge_font_plist(
                                &mut contribution,
                                items,
                                remapping,
                                seen,
                                depth + 1,
                            );
                        }
                    }
                }
                seen.pop();
                *base = base.merge(&contribution);
            } else if let Some(face) = self.faces.get(&reference) {
                if let Some(inherit) = face.inherit {
                    self.merge_font_reference(base, inherit, remapping, seen, depth + 1);
                }
                *base = base.merge(face);
            }
            if normal {
                seen.pop();
            }
        } else if let Some(items) = list_to_vec(&reference) {
            if items.first().is_some_and(|value| value.is_keyword()) {
                self.merge_font_plist(base, &items, remapping, seen, depth + 1);
            } else {
                for item in items.iter().rev() {
                    self.merge_font_reference(base, *item, remapping, seen, depth + 1);
                }
            }
        }
    }

    /// GNU push_named_merge_point: remaps hide earlier normal merge points,
    /// but a second point of the same kind is a cycle. Stack entries are
    /// path-scoped, permitting repeated references in independent list arms.
    fn push_font_merge_point(seen: &mut Vec<(Value, bool)>, name: Value, remap: bool) -> bool {
        for &(previous, previous_remap) in seen.iter().rev() {
            if previous == name {
                if previous_remap == remap {
                    return false;
                }
                if previous_remap {
                    break;
                }
            }
        }
        seen.push((name, remap));
        true
    }

    fn merge_font_plist(
        &self,
        base: &mut Face,
        items: &[Value],
        remapping: &FaceRemapping,
        seen: &mut Vec<(Value, bool)>,
        depth: usize,
    ) {
        // Anonymous face refs process :inherit at its written position,
        // unlike named vectors, which always process inheritance first.
        for pair in items.chunks_exact(2) {
            if pair[0] == Value::keyword("inherit") {
                self.merge_font_reference(base, pair[1], remapping, seen, depth + 1);
            } else {
                *base = base.merge(&Face::from_plist("--font--", pair));
            }
        }
    }

    /// Resolve a text-face contribution, including explicit inheritance but
    /// without injecting the default face into an existing text/overlay base.
    pub(crate) fn resolve_text_face_with_remapping(
        &self,
        name: &str,
        remapping: &FaceRemapping,
    ) -> Face {
        self.resolve_remapped(
            name,
            remapping,
            &mut HashSet::new(),
            0,
            RemappingBase::InheritedOnly,
        )
    }

    /// Resolve a named face with GNU's highest-priority-first remapping list.
    pub fn resolve_with_remapping(&self, name: &str, remapping: &FaceRemapping) -> Face {
        self.resolve_remapped(
            name,
            remapping,
            &mut HashSet::new(),
            0,
            RemappingBase::DefaultAndInherited,
        )
    }

    /// Overlay named text faces on the remapped default face. The caller's
    /// text-face order is preserved; each individual remapping uses GNU's
    /// highest-priority-first order.
    pub fn merge_faces_with_remapping(
        &self,
        face_names: &[&str],
        remapping: &FaceRemapping,
    ) -> Face {
        let mut result = self.resolve_with_remapping("default", remapping);
        for name in face_names {
            let resolved = self.resolve_remapped(
                name,
                remapping,
                &mut HashSet::new(),
                0,
                RemappingBase::SpecifiedOnly,
            );
            result = result.merge(&resolved);
        }
        result
    }

    fn resolve_remapped(
        &self,
        name: &str,
        remapping: &FaceRemapping,
        seen: &mut HashSet<Value>,
        depth: usize,
        base: RemappingBase,
    ) -> Face {
        if depth > 20 {
            return Face::new(name);
        }
        let key = face_symbol_value(name);
        if !seen.contains(&key)
            && let Some(entries) = remapping.get(name)
        {
            seen.insert(key);
            let mut result = match base {
                RemappingBase::DefaultAndInherited => self.resolve("default"),
                RemappingBase::SpecifiedOnly | RemappingBase::InheritedOnly => Face::new(name),
            };
            // GNU xfaces.c merge_face_ref merges the tail first: the first
            // entry wins, and a trailing self-reference supplies the original
            // definition without overwriting higher-priority relative heights.
            for entry in entries.iter().rev() {
                let contribution = match entry {
                    FaceRemapEntry::RemapFace(target) => {
                        let Some(target_name) = target.as_symbol_name() else {
                            continue;
                        };
                        self.resolve_remapped(target_name, remapping, seen, depth + 1, base)
                    }
                    FaceRemapEntry::RemapAttrs(attrs, _) => attrs.clone(),
                };
                result = result.merge(&contribution);
            }
            return result;
        }
        match base {
            RemappingBase::DefaultAndInherited | RemappingBase::InheritedOnly => {
                self.resolve_depth(name, 0)
            }
            RemappingBase::SpecifiedOnly => self
                .faces
                .get(&key)
                .cloned()
                .unwrap_or_else(|| Face::new(name)),
        }
    }
}
