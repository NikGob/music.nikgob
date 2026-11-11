//! How a change of the preferences made on a copy reaches the preferences
//! themselves, and how a prefs file is read when part of it does not fit.
//!
//! The settings page edits a copy of the prefs it was handed this frame.
//! The runtime applies commands one after another on its own thread and
//! is often a little behind the page (one change of a slider's drag is a
//! command a frame, each one saved to disk): the copy can be older than a
//! change still on its way. Sending the whole copy back took that change
//! back — the text size set a moment before went back to what it was when
//! the next row moved. [`Prefs::merge_edit`] lays only the fields the page
//! changed over the prefs as they are when the edit comes.

use crate::Prefs;

impl Prefs {
    /// Every field `after` has unlike `before` laid over `self`; the
    /// others stay as `self` has them. Field by field as the prefs file
    /// names them, so a field added later is covered without a list here.
    pub fn merge_edit(&mut self, before: &Prefs, after: &Prefs) {
        use serde_json::Value;
        let (Ok(Value::Object(was)), Ok(Value::Object(now)), Ok(Value::Object(mut merged))) =
            (serde_json::to_value(before), serde_json::to_value(after), serde_json::to_value(&*self))
        else {
            *self = after.clone();
            return;
        };
        let mut changed = false;
        for (key, value) in now {
            if was.get(&key) != Some(&value) {
                merged.insert(key, value);
                changed = true;
            }
        }
        if !changed {
            return;
        }
        match serde_json::from_value::<Prefs>(Value::Object(merged)) {
            Ok(p) => *self = p,
            // (A value that cannot be written as JSON, a NaN say.)
            Err(_) => *self = after.clone(),
        }
    }

    /// The prefs of a prefs file read as JSON: as a whole when it reads,
    /// else field by field — a field that does not read (written by
    /// another build, a value this one does not know) takes its default
    /// and every other keeps the user's value. Before, one such field threw
    /// the whole file away for the defaults.
    pub fn from_json_lenient(value: serde_json::Value) -> serde_json::Result<Prefs> {
        use serde_json::Value;
        let e = match serde_json::from_value::<Prefs>(value.clone()) {
            Ok(p) => return Ok(p),
            Err(e) => e,
        };
        let Value::Object(file) = value else { return Err(e) };
        let mut kept = serde_json::Map::new();
        for (key, v) in file {
            let mut one = serde_json::Map::new();
            one.insert(key.clone(), v.clone());
            if serde_json::from_value::<Prefs>(Value::Object(one)).is_ok() {
                kept.insert(key, v);
            }
        }
        serde_json::from_value(Value::Object(kept))
    }
}

#[cfg(test)]
mod tests {
    use crate::{PrefPatch, Prefs};

    /// The page changed one row on a copy from before the size was set
    /// (the runtime had not got that far when the page drew): the size the
    /// user set stays, and the row's change is made. Sending the whole copy
    /// (`SetPrefs`) put the old size back.
    #[test]
    fn an_edit_on_an_older_copy_keeps_the_newer_change() {
        let base = Prefs::default();
        let mut runtime = Prefs { lyrics_scale: 0.9, ..base.clone() };
        let page = Prefs { lyrics_dim: 0.3, ..base.clone() };
        // What the whole copy did (`SetPrefs` replaces the prefs).
        let whole = page.clone();
        assert_eq!(whole.lyrics_scale, 1.0, "the old way loses the size");
        PrefPatch::Edit { before: Box::new(base.clone()), after: Box::new(page) }.apply(&mut runtime);
        assert_eq!(runtime.lyrics_scale, 0.9, "the size set before stays");
        assert_eq!(runtime.lyrics_dim, 0.3, "the row's change is made");
        // Nothing changed on the page: nothing changes.
        let before = runtime.clone();
        PrefPatch::Edit { before: Box::new(base.clone()), after: Box::new(base) }.apply(&mut runtime);
        assert_eq!(runtime, before);
    }

    /// Maps and lists go as one field: an edit of the presets kept for
    /// «Свои» or of the menu's items is the page's whole new value.
    #[test]
    fn an_edit_of_a_map_takes_the_page_s_value() {
        use crate::presets::Area;
        let base = Prefs::default();
        let mut page = base.clone();
        Area::Load.apply(&mut page, 0);
        let mut runtime = Prefs { lyrics_scale: 0.85, ..base.clone() };
        runtime.merge_edit(&base, &page);
        assert_eq!(runtime.custom_presets, page.custom_presets);
        assert_eq!(runtime.fps_limit, page.fps_limit);
        assert_eq!(runtime.lyrics_scale, 0.85);
    }

    /// A field this build cannot read takes its default; the rest of the
    /// file is the user's.
    #[test]
    fn a_field_that_does_not_read_keeps_the_others() {
        let mut file = serde_json::to_value(Prefs { lyrics_scale: 0.9, fps_limit: 142, ..Prefs::default() }).unwrap();
        file["theme"] = serde_json::Value::String("NoSuchTheme".into());
        file["lyrics_dim"] = serde_json::Value::String("much".into());
        assert!(serde_json::from_value::<Prefs>(file.clone()).is_err(), "as a whole it does not read");
        let p = Prefs::from_json_lenient(file).unwrap();
        assert_eq!(p.lyrics_scale, 0.9);
        assert_eq!(p.fps_limit, 142);
        assert_eq!(p.theme, Prefs::default().theme);
        assert_eq!(p.lyrics_dim, Prefs::default().lyrics_dim);
        // A file that reads is read as it is.
        let fine = serde_json::to_value(Prefs { lyrics_scale: 0.9, ..Prefs::default() }).unwrap();
        assert_eq!(Prefs::from_json_lenient(fine).unwrap().lyrics_scale, 0.9);
    }

    /// 100 % for a new user and for a field missing from old prefs; the
    /// user's own value is read back as it was saved.
    #[test]
    fn the_text_size_is_100_by_default_and_a_saved_one_stays() {
        assert_eq!(Prefs::default().lyrics_scale, 1.0);
        assert_eq!(Prefs::first_start().lyrics_scale, 1.0);
        let saved = serde_json::to_value(Prefs { lyrics_scale: 0.9, ..Prefs::first_start() }).unwrap();
        assert_eq!(Prefs::from_json_lenient(saved).unwrap().lyrics_scale, 0.9);
    }
}
