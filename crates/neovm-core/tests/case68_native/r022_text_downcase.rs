//! #68 item 1: GNU-executed syntax-aware text downcase regressions.
//! Exact forms/results: r22/native/gnu-fixture-binding/results.json.

#[test]
fn capitalize_final_sigma_string() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"(capitalize "ΟΣ Σ ΟΣΑ ΟΣ. 1Σ AΣ_Β")"#,
    );
    assert_eq!(result, r#"OK "Ος Σ Οσα Ος. 1ς Aς_Β""#);
}

#[test]
fn capitalize_final_sigma_region() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"(with-temp-buffer (insert "ΟΣ Σ ΟΣΑ ΟΣ. 1Σ AΣ_Β") (capitalize-region 1 (point-max)) (list (buffer-string) (point) (point-max)))"#,
    );
    assert_eq!(result, r#"OK ("Ος Σ Οσα Ος. 1ς Aς_Β" 21 21)"#);
}

#[test]
fn capitalize_final_sigma_word() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"(with-temp-buffer (insert "ΟΣ") (goto-char 1) (capitalize-word 1) (list (buffer-string) (point) (point-max)))"#,
    );
    assert_eq!(result, r#"OK ("Ος" 3 3)"#);
}

#[test]
fn capitalize_standard_kelvin_string() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"(list (capitalize "AKB") (capitalize "ΑKΒ") (downcase "AKB") (downcase ?K))"#,
    );
    assert_eq!(result, r#"OK ("AKb" "ΑKβ" "aKb" 8490)"#);
}

#[test]
fn capitalize_standard_kelvin_region() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"(with-temp-buffer (insert "AKB ΑKΒ") (capitalize-region 1 (point-max)) (list (buffer-string) (point) (point-max)))"#,
    );
    assert_eq!(result, r#"OK ("AKb ΑKβ" 8 8)"#);
}

#[test]
fn capitalize_word_kelvin_gnu_boundary() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"(mapcar (lambda (s) (with-temp-buffer (insert s) (goto-char 1) (capitalize-word 1) (list (buffer-string) (point) (point-max)))) (list "AKB" "ΑKΒ"))"#,
    );
    assert_eq!(result, r#"OK (("AKB" 2 4) ("ΑKΒ" 2 4))"#);
}

#[test]
fn custom_sigma_changed_string() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"(with-temp-buffer (let ((tbl (copy-case-table (standard-case-table)))) (aset tbl ?Σ ?x) (set-case-table tbl)) (list (capitalize "AΣ AΣB Σ") (downcase "AΣ AΣB Σ") (downcase ?Σ)))"#,
    );
    assert_eq!(result, r#"OK ("Aς Axb Σ" "aς axb x" 120)"#);
}

#[test]
fn custom_sigma_changed_region_word() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"(mapcar (lambda (op) (with-temp-buffer (let ((tbl (copy-case-table (standard-case-table)))) (aset tbl ?Σ ?x) (set-case-table tbl)) (insert "AΣ") (goto-char 1) (if (eq op 'capitalize-word) (capitalize-word 1) (funcall op 1 (point-max))) (list (buffer-string) (point) (point-max)))) '(capitalize-region capitalize-word downcase-region))"#,
    );
    assert_eq!(result, r#"OK (("Aς" 1 3) ("AΣ" 2 3) ("aς" 1 3))"#);
}

#[test]
fn custom_sigma_identity_string() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"(with-temp-buffer (let ((tbl (copy-case-table (standard-case-table)))) (aset tbl ?Σ ?Σ) (set-case-table tbl)) (list (capitalize "AΣ AΣB Σ") (downcase "AΣ AΣB Σ") (downcase ?Σ)))"#,
    );
    assert_eq!(result, r#"OK ("AΣ AΣb Σ" "aΣ aΣb Σ" 931)"#);
}

#[test]
fn custom_sigma_identity_region_word() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"(mapcar (lambda (op) (with-temp-buffer (let ((tbl (copy-case-table (standard-case-table)))) (aset tbl ?Σ ?Σ) (set-case-table tbl)) (insert "AΣ") (goto-char 1) (if (eq op 'capitalize-word) (capitalize-word 1) (funcall op 1 (point-max))) (list (buffer-string) (point) (point-max)))) '(capitalize-region capitalize-word downcase-region))"#,
    );
    assert_eq!(result, r#"OK (("AΣ" 1 3) ("AΣ" 2 3) ("aΣ" 1 3))"#);
}

#[test]
fn custom_sigma_nil_string() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"(with-temp-buffer (let ((tbl (copy-case-table (standard-case-table)))) (aset tbl ?Σ nil) (set-case-table tbl)) (list (capitalize "AΣ AΣB Σ") (downcase "AΣ AΣB Σ") (downcase ?Σ)))"#,
    );
    assert_eq!(result, r#"OK ("AΣ AΣb Σ" "aΣ aΣb Σ" 931)"#);
}

#[test]
fn custom_sigma_nil_region_word() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"(mapcar (lambda (op) (with-temp-buffer (let ((tbl (copy-case-table (standard-case-table)))) (aset tbl ?Σ nil) (set-case-table tbl)) (insert "AΣ") (goto-char 1) (if (eq op 'capitalize-word) (capitalize-word 1) (funcall op 1 (point-max))) (list (buffer-string) (point) (point-max)))) '(capitalize-region capitalize-word downcase-region))"#,
    );
    assert_eq!(result, r#"OK (("AΣ" 1 3) ("AΣ" 2 3) ("aΣ" 1 3))"#);
}

#[test]
fn custom_kelvin_explicit_and_nil() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"(mapcar (lambda (mapping) (with-temp-buffer (let ((tbl (copy-case-table (standard-case-table)))) (aset tbl ?K mapping) (set-case-table tbl)) (list (capitalize "AKB") (downcase "AKB") (downcase ?K)))) (list ?k nil))"#,
    );
    assert_eq!(result, r#"OK (("Akb" "akb" 107) ("AKb" "aKb" 8490))"#);
}

#[test]
fn special_lowercase_beats_custom_simple_string() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"(with-temp-buffer (let ((tbl (copy-case-table (standard-case-table)))) (aset tbl ?İ ?x) (set-case-table tbl)) (list (capitalize "AİB") (downcase "AİB") (downcase ?İ) (upcase-initials "AİB")))"#,
    );
    assert_eq!(result, r#"OK ("Ai̇b" "ai̇b" 120 "AİB")"#);
}

#[test]
fn special_lowercase_beats_custom_simple_region_word() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"(mapcar (lambda (op) (with-temp-buffer (let ((tbl (copy-case-table (standard-case-table)))) (aset tbl ?İ ?x) (set-case-table tbl)) (insert "AİB") (goto-char 1) (if (eq op 'capitalize-word) (capitalize-word 1) (funcall op 1 (point-max))) (list (buffer-string) (point) (point-max)))) '(capitalize-region capitalize-word downcase-region upcase-initials-region))"#,
    );
    assert_eq!(result, r#"OK (("Ai̇b" 1 5) ("Ai̇b" 5 5) ("ai̇b" 1 5) ("AİB" 1 4))"#);
}

#[test]
fn sigma_lookahead_respects_symbol_syntax() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"(mapcar (lambda (flag) (let ((case-symbols-as-words flag)) (list (capitalize "AΣ_B A_Σ") (downcase "AΣ_B A_Σ") (with-temp-buffer (insert "AΣ_B A_Σ") (capitalize-region 1 (point-max)) (buffer-string))))) '(nil t))"#,
    );
    assert_eq!(result, r#"OK (("Aς_B A_Σ" "aς_b a_σ" "Aς_B A_Σ") ("Aσ_b A_ς" "aσ_b a_ς" "Aσ_b A_ς"))"#);
}

#[test]
fn sigma_lookahead_uses_original_custom_syntax() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"(with-temp-buffer (modify-syntax-entry ?! "w") (modify-syntax-entry ?B ".") (list (capitalize "AΣ! AΣB !Σ") (downcase "AΣ! AΣB !Σ")))"#,
    );
    assert_eq!(result, r#"OK ("Aσ! Aςb !ς" "aσ! aςb !ς")"#);
}

#[test]
fn capitalize_prefix_word_state_and_initials_control() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"(with-temp-buffer (modify-syntax-entry ?A "w p") (list (capitalize "AΣ") (with-temp-buffer (modify-syntax-entry ?A "w p") (insert "AΣ") (capitalize-region 1 (point-max)) (buffer-string)) (upcase-initials "ΟΣ AİB AKB")))"#,
    );
    assert_eq!(result, r#"OK ("Aς" "AΣ" "ΟΣ AİB AKB")"#);
}

// GNU r22 oracle: word-count-3-AKB
#[test]
fn boundary_oracle_1_word_count_3_AkelvinB() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"(with-temp-buffer (progn (insert "AKB") (goto-char (point-min)) (capitalize-word 3) (list (buffer-string) (point))))"#,
    );
    assert_eq!(result, r#"OK ("AKb" 4)"#);
}

// GNU r22 oracle: word-count-3-ΑKΒ
#[test]
fn boundary_oracle_2_word_count_3_alphakelvinbeta() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"(with-temp-buffer (progn (insert "ΑKΒ") (goto-char (point-min)) (capitalize-word 3) (list (buffer-string) (point))))"#,
    );
    assert_eq!(result, r#"OK ("ΑKβ" 4)"#);
}

// GNU r22 oracle: word-count-3-AΒC
#[test]
fn boundary_oracle_3_word_count_3_AbetaC() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"(with-temp-buffer (progn (insert "AΒC") (goto-char (point-min)) (capitalize-word 3) (list (buffer-string) (point))))"#,
    );
    assert_eq!(result, r#"OK ("Aβc" 4)"#);
}

// GNU r22 oracle: boundary-combine-all
#[test]
fn boundary_oracle_4_boundary_combine_all() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"(let ((word-combining-categories '((nil . nil))))  (list
(with-temp-buffer (insert "AKB") (goto-char 1) (forward-word 1) (point))
(with-temp-buffer (insert "AKB") (goto-char 1) (capitalize-word 1) (list (buffer-string) (point)))
(capitalize "AKB")))"#,
    );
    assert_eq!(result, r#"OK (4 ("AKb" 4) "AKb")"#);
}

// GNU r22 oracle: boundary-same-script
#[test]
fn boundary_oracle_5_boundary_same_script() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"(let ((char-script-table (copy-sequence char-script-table))) (set-char-table-range char-script-table ?K 'latin) (list
(with-temp-buffer (insert "AKB") (goto-char 1) (forward-word 1) (point))
(with-temp-buffer (insert "AKB") (goto-char 1) (capitalize-word 1) (list (buffer-string) (point)))
(capitalize "AKB")))"#,
    );
    assert_eq!(result, r#"OK (4 ("AKb" 4) "AKb")"#);
}

// GNU r22 oracle: extra-same-script-separating-all
#[test]
fn boundary_oracle_6_extra_same_script_separating_all() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"(let ((char-script-table (copy-sequence char-script-table)) (word-separating-categories '((nil . nil)))) (set-char-table-range char-script-table ?K 'latin) (with-temp-buffer (insert "AKB") (goto-char 1) (capitalize-word 1) (list (buffer-string) (point))))"#,
    );
    assert_eq!(result, r#"OK ("AKB" 2)"#);
}

// GNU r22 oracle: extra-combine-directed
#[test]
fn boundary_oracle_7_extra_combine_directed() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"(let ((word-combining-categories '((?l . ?5)))) (with-temp-buffer (insert "AKB") (goto-char 1) (capitalize-word 1) (list (buffer-string) (point))))"#,
    );
    assert_eq!(result, r#"OK ("AKB" 3)"#);
}

// GNU r22 oracle: extra-symbols-word-count3
#[test]
fn boundary_oracle_8_extra_symbols_word_count3() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"(let ((case-symbols-as-words t)) (with-temp-buffer (insert "ΑΣ_") (goto-char 1) (capitalize-word 3) (list (buffer-string) (point))))"#,
    );
    assert_eq!(result, r#"OK ("Ασ_" 4)"#);
}

// GNU r22 oracle: extra-sigma-script-truncated-word
#[test]
fn boundary_oracle_9_extra_sigma_script_truncated_word() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"(with-temp-buffer (insert "ΑΣA") (goto-char 1) (capitalize-word 1) (list (buffer-string) (point)))"#,
    );
    assert_eq!(result, r#"OK ("ΑςA" 3)"#);
}

// GNU r22 oracle: extra-sigma-script-truncated-region
#[test]
fn boundary_oracle_10_extra_sigma_script_truncated_region() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"(with-temp-buffer (insert "ΑΣA") (capitalize-region 1 3) (buffer-string))"#,
    );
    assert_eq!(result, r#"OK "ΑςA""#);
}

// GNU r22 oracle: extra-sigma-script-full-string
#[test]
fn boundary_oracle_11_extra_sigma_script_full_string() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"(capitalize "ΑΣA")"#,
    );
    assert_eq!(result, r#"OK "Ασa""#);
}

// GNU r22 oracle: extra-kelvin-k-word-count1
#[test]
fn boundary_oracle_12_extra_kelvin_k_word_count1() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"(with-temp-buffer (set-case-table (copy-case-table (standard-case-table))) (set-char-table-range (current-case-table) ?K ?k) (insert "AKB") (goto-char 1) (capitalize-word 1) (list (buffer-string) (point)))"#,
    );
    assert_eq!(result, r#"OK ("AKB" 2)"#);
}

// GNU r22 oracle: extra-sigma-x-nonfinal-word
#[test]
fn boundary_oracle_13_extra_sigma_x_nonfinal_word() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"(with-temp-buffer (set-case-table (copy-case-table (standard-case-table))) (set-char-table-range (current-case-table) ?Σ ?x) (progn (insert "ΑΣΑ") (goto-char 1) (capitalize-word 1) (list (buffer-string) (point))))"#,
    );
    assert_eq!(result, r#"OK ("Αxα" 4)"#);
}

// GNU r22 oracle: sigma-symbols-t
#[test]
fn boundary_oracle_14_sigma_symbols_t() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"(let ((case-symbols-as-words t)) (list (capitalize "ΑΣ_")
(with-temp-buffer (insert "ΑΣ_") (capitalize-region 1 (point-max)) (buffer-string))
(with-temp-buffer (insert "ΑΣ_") (goto-char 1) (capitalize-word 1) (list (buffer-string) (point)))))"#,
    );
    assert_eq!(result, r#"OK ("Ασ_" "Ασ_" ("Ας_" 3))"#);
}
