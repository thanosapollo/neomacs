//! Selected original casetab fixtures; bodies and assertions are verbatim.
//! See writer/inventory.json for original identities and exclusions.

use neovm_core::case68_test_support::{make_case_table_with_pair, builtin_set_case_table};
use neovm_core::Value;

mod tests {
use super::*;

#[cfg(test)]
fn ctx_with_pair(uc: i64, lc: i64) -> super::super::eval::Context {
    let mut ev = super::super::eval::Context::new();
    let table = make_case_table_with_pair(uc, lc);
    builtin_set_case_table(&mut ev, vec![table]).expect("set-case-table");
    ev
}

#[test]
fn custom_case_table_overrides_upcase_region() {
    crate::test_utils::init_test_tracing();
    // Upcasing "Az" with the z/A pair: ?A upcases to ?z, ?z stays ?z => "zz".
    let mut ev = ctx_with_pair('z' as i64, 'A' as i64);
    let buffer_id = ev.buffers.current_buffer_id().expect("current buffer");
    ev.buffers.insert_into_buffer(buffer_id, "Az");
    super::super::casefiddle::builtin_upcase_region(
        &mut ev,
        vec![Value::fixnum(1), Value::fixnum(3)],
    )
    .expect("upcase-region");
    let buffer = ev.buffers.get(buffer_id).expect("buffer");
    assert_eq!(buffer.buffer_string(), "zz");
}

#[test]
fn the_standard_case_tables_and_replace_match_case_answer_like_gnu() {
    crate::test_utils::init_test_tracing();
    // GNU 31.1: every subsidiary carries the three extra slots, canon's
    // third is eqv, and `replace-match' carries the match's case over.
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"
        (let* ((st (standard-case-table))
               (up (char-table-extra-slot st 0))
               (canon (char-table-extra-slot st 1))
               (eqv (char-table-extra-slot st 2)))
          (list (aref st ?A) (aref st ?a) (aref up ?a) (aref up ?1) (aref canon ?Q)
                (aref eqv ?q) (aref eqv ?Q) (aref st ?É) (aref up ?é) (aref canon ?Ω)
                (aref st 1)
                (eq (char-table-extra-slot canon 2) eqv)
                (length (list (char-table-extra-slot up 0) (char-table-extra-slot up 2)))
                (with-temp-buffer
                  (insert "Hello World and HELLO WORLD and hello world and Émile")
                  (let ((case-fold-search t))
                    (goto-char 1)
                    (while (re-search-forward "hello world\\|émile" nil t)
                      (replace-match
                       (if (string-match-p "mile" (match-string 0)) "zoé" "bye there"))))
                  (buffer-string))))
        "#,
    );
    assert_eq!(
        result,
        "OK (97 97 65 49 113 81 113 233 201 969 1 t 2 \"Bye There and BYE THERE and bye there and Zoé\")"
    );
}


#[test]
fn custom_case_table_overrides_upcase_downcase_char() {
    crate::test_utils::init_test_tracing();
    // (set-case-syntax-pair ?z ?A): ?z is the uppercase, ?A the lowercase.
    // GNU oracle: (upcase ?z) => 122, (downcase ?A) => 65.
    let mut ev = ctx_with_pair('z' as i64, 'A' as i64);
    let up = crate::emacs_core::builtins::builtin_upcase_in_state(&mut ev, vec![Value::char('z')])
        .unwrap();
    assert_eq!(up.as_int(), Some(122));
    let down =
        crate::emacs_core::builtins::builtin_downcase_in_state(&mut ev, vec![Value::char('A')])
            .unwrap();
    assert_eq!(down.as_int(), Some(65));
}

#[test]
fn custom_case_table_overrides_upcase_downcase_string() {
    crate::test_utils::init_test_tracing();
    let mut ev = ctx_with_pair('z' as i64, 'A' as i64);
    // upcase "Az" => "zz" (A->z, z->z).
    let up =
        crate::emacs_core::builtins::builtin_upcase_in_state(&mut ev, vec![Value::string("Az")])
            .unwrap();
    assert_eq!(up.as_utf8_str(), Some("zz"));
    // downcase "Az" => "AA" (A->A, z->A).
    let down =
        crate::emacs_core::builtins::builtin_downcase_in_state(&mut ev, vec![Value::string("Az")])
            .unwrap();
    assert_eq!(down.as_utf8_str(), Some("AA"));
}

#[test]
fn custom_case_table_makes_char_equal_case_fold() {
    crate::test_utils::init_test_tracing();
    // (set-case-syntax-pair ?x ?Y): ?x/?Y become a case pair, so with
    // case-fold-search t, (char-equal ?x ?Y) => t. GNU oracle: t.
    let mut ev = ctx_with_pair('x' as i64, 'Y' as i64);
    ev.set_variable("case-fold-search", Value::T);
    let r = crate::emacs_core::builtins::builtin_char_equal(
        &mut ev,
        vec![Value::char('x'), Value::char('Y')],
    )
    .unwrap();
    assert!(r.is_t(), "expected t, got {r:?}");

    // case-fold-search nil: exact match only.
    ev.set_variable("case-fold-search", Value::NIL);
    let r = crate::emacs_core::builtins::builtin_char_equal(
        &mut ev,
        vec![Value::char('x'), Value::char('Y')],
    )
    .unwrap();
    assert!(r.is_nil(), "expected nil, got {r:?}");
}

#[test]
fn standard_case_table_path_is_byte_identical() {
    crate::test_utils::init_test_tracing();
    // Without a custom table, the default Unicode path must be unchanged.
    let mut ev = super::super::eval::Context::new();
    let up = crate::emacs_core::builtins::builtin_upcase_in_state(&mut ev, vec![Value::char('z')])
        .unwrap();
    assert_eq!(up.as_int(), Some('Z' as i64));
    let down =
        crate::emacs_core::builtins::builtin_downcase_in_state(&mut ev, vec![Value::char('A')])
            .unwrap();
    assert_eq!(down.as_int(), Some('a' as i64));
    // Non-ASCII Unicode casing still works (é <-> É).
    let up =
        crate::emacs_core::builtins::builtin_upcase_in_state(&mut ev, vec![Value::string("héllo")])
            .unwrap();
    assert_eq!(up.as_utf8_str(), Some("HÉLLO"));
    let down = crate::emacs_core::builtins::builtin_downcase_in_state(
        &mut ev,
        vec![Value::string("HÉLLO")],
    )
    .unwrap();
    assert_eq!(down.as_utf8_str(), Some("héllo"));
    // char-equal still case-folds with the standard table.
    ev.set_variable("case-fold-search", Value::T);
    let r = crate::emacs_core::builtins::builtin_char_equal(
        &mut ev,
        vec![Value::char('a'), Value::char('A')],
    )
    .unwrap();
    assert!(r.is_t());
}
}
