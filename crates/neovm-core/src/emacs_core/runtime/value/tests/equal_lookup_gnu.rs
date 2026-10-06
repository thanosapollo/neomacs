//! The GNU oracle forms of `neovm-oracle-tests`
//! `hash/equal_lookup_bounded_semantics.rs`, run in process against the
//! answers GNU 31.1 gave there (`NEOVM_ORACLE_MODE=refresh UPDATE_EXPECT=1`):
//! the oracle suite runs the release binary, this runs the same forms
//! through the evaluator a unit test can build.
use crate::test_utils::{oracle_expect_transcript, runtime_startup_eval_one};

#[test]
fn equal_hash_long_and_huge_keys_are_found() {
    crate::test_utils::init_test_tracing();
    let form = r#"
(let* ((h (make-hash-table :test 'equal))
       (big (make-vector 100000 0))
       (key (list 1 big)))
  (puthash (number-sequence 1 300) 'long h)
  (puthash key 'huge h)
  (list (gethash (number-sequence 1 300) h)
        (gethash (number-sequence 1 301) h)
        (gethash key h)
        (gethash (list 1 (make-vector 100000 0)) h)
        (gethash (list 1 (make-vector 99999 0)) h)
        (let ((copy (make-vector 100000 0)))
          (aset copy 99999 1)
          (gethash (list 1 copy) h))
        (hash-table-count h)))
"#;
    assert_eq!(
        runtime_startup_eval_one(form),
        oracle_expect_transcript(r#""OK (long nil huge huge nil nil 2)""#)
    );
}

#[test]
fn equal_hash_keys_sharing_the_hashed_prefix_stay_distinct() {
    crate::test_utils::init_test_tracing();
    let form = r#"
(let ((h (make-hash-table :test 'equal))
      (v (make-hash-table :test 'equal))
      (i 0))
  (while (< i 50)
    (puthash (append (make-list 30 'x) (list i)) i h)
    (let ((vec (make-vector 12 0)))
      (aset vec 11 i)
      (puthash vec (* 2 i) v))
    (setq i (1+ i)))
  (list (hash-table-count h)
        (gethash (append (make-list 30 'x) (list 42)) h)
        (gethash (append (make-list 30 'x) (list 99)) h)
        (hash-table-count v)
        (let ((vec (make-vector 12 0))) (aset vec 11 7) (gethash vec v))
        (let ((vec (make-vector 12 0))) (aset vec 11 70) (gethash vec v 'none))
        (progn (remhash (append (make-list 30 'x) (list 42)) h)
               (list (hash-table-count h)
                     (gethash (append (make-list 30 'x) (list 42)) h 'gone)
                     (gethash (append (make-list 30 'x) (list 41)) h)))))
"#;
    assert_eq!(
        runtime_startup_eval_one(form),
        oracle_expect_transcript(r#""OK (50 42 nil 50 14 none (49 gone 41))""#)
    );
}

#[test]
fn equal_hash_mutated_keys_compare_the_live_object() {
    crate::test_utils::init_test_tracing();
    let form = r#"
(let ((h (make-hash-table :test 'equal))
      (k (list 'a 'b))
      (w (make-hash-table :test 'equal))
      (vec (make-vector 20 0))
      (p (make-hash-table :test 'equal))
      (pk (list 1 2)))
  (puthash k 8 h)
  (setcar k 'z)
  (puthash vec 'v w)
  ;; Slot 15 lies past SXHASH_MAX_LEN, so the key keeps its hash.
  (aset vec 15 9)
  (puthash pk 'old p)
  (setcar pk 5)
  (puthash (list 1 2) 'new p)
  (list (gethash (list 'a 'b) h)
        (gethash (list 'z 'b) h)
        (gethash vec w)
        (gethash (make-vector 20 0) w)
        (let ((copy (make-vector 20 0))) (aset copy 15 9) (gethash copy w))
        (hash-table-count p)
        (gethash (list 1 2) p)
        (let (keys) (maphash (lambda (key _) (push key keys)) p) (nreverse keys))))
"#;
    assert_eq!(
        runtime_startup_eval_one(form),
        oracle_expect_transcript(r#""OK (nil nil v nil v 2 new ((5 2) (1 2)))""#)
    );
}

#[test]
fn equal_hash_circular_and_deep_keys() {
    crate::test_utils::init_test_tracing();
    let form = r#"
(let ((h (make-hash-table :test 'equal))
      (k (list 1 2 3))
      (k2 (list 1 2 3))
      (a 1)
      (b 1))
  (setcdr (cddr k) k)
  (setcdr (cddr k2) k2)
  (puthash k 'cycle h)
  (dotimes (_ 300) (setq a (list a) b (list b)))
  (puthash a 'deep h)
  (list (gethash k h)
        (gethash k2 h)
        (gethash a h)
        (condition-case err (gethash b h) (error (list 'error err)))
        (condition-case err (progn (puthash b 'other h) 'stored) (error (list 'error err)))
        (condition-case err (progn (remhash b h) 'removed) (error (list 'error err)))
        (hash-table-count h)))
"#;
    assert_eq!(
        runtime_startup_eval_one(form),
        oracle_expect_transcript(
            r#""OK (cycle cycle deep (error (error \"Stack overflow in equal\")) (error (error \"Stack overflow in equal\")) (error (error \"Stack overflow in equal\")) 2)""#
        )
    );
}

#[test]
fn equal_hash_iteration_and_printing_keep_insertion_order() {
    crate::test_utils::init_test_tracing();
    let form = r#"
(let ((h (make-hash-table :test 'equal)) keys)
  (dolist (k (list '(a) '(b c) [1 2] "s" 1.5 (number-sequence 1 10) 'sym 7))
    (puthash k (length (format "%S" k)) h))
  (remhash '(b c) h)
  (puthash (list 'late) 0 h)
  (maphash (lambda (k v) (push (cons k v) keys)) h)
  (list (nreverse keys)
        (hash-table-count h)
        (prin1-to-string h)))
"#;
    assert_eq!(
        runtime_startup_eval_one(form),
        oracle_expect_transcript(
            r##""OK ((((a) . 3) ((late) . 0) ([1 2] . 5) (\"s\" . 3) (1.5 . 3) ((1 2 3 4 5 6 7 8 9 10) . 22) (sym . 3) (7 . 1)) 8 \"#s(hash-table test equal data ((a) 3 (late) 0 [1 2] 5 \\\"s\\\" 3 1.5 3 (1 2 3 4 5 6 7 8 9 10) 22 sym 3 7 1))\")""##
        )
    );
}

/// A key reached by 2^64 paths through shared structure is inserted,
/// found, iterated and removed in bounded time and memory, as GNU does:
/// `sxhash` looks at a bounded prefix and lookups compare the live key.
/// Materializing the stored key as a tree copied every shared node once per
/// path, so the profiler's `equal` log of closure backtraces (closures share
/// their captured environments) exhausted memory.
#[test]
fn equal_hash_keys_with_shared_substructure_stay_bounded() {
    crate::test_utils::init_test_tracing();
    let form = r#"
(let* ((h (make-hash-table :test 'equal))
       (x (list 1)))
  (dotimes (_ 64) (setq x (list x x)))
  (let* ((f (let ((env x)) (lambda () env)))
         (key (vector f f)))
    (puthash x 'dag h)
    (puthash key 'closure h)
    (puthash (list 'small x) 'small h)
    (list (gethash x h) (gethash key h) (gethash (list 'small x) h)
          (gethash (list 'other x) h) (hash-table-count h)
          (let (n) (maphash (lambda (k _v) (push (eq k x) n)) h) (nreverse n))
          (progn (remhash x h) (list (gethash x h 'gone) (hash-table-count h)))
          (hash-table-count (copy-hash-table h)))))
"#;
    assert_eq!(
        runtime_startup_eval_one(form),
        oracle_expect_transcript(r#""OK (dag closure small nil 3 (t nil nil) (gone 2) 2)""#)
    );
}

/// Rust maps keyed by `Value` (menu ancestors, text-property maps) hash
/// with the bounded `equal` hash too, so a value with shared substructure
/// hashes in bounded time and still finds its `equal` twin.
#[test]
fn value_hash_of_shared_substructure_is_bounded_and_equal_consistent() {
    crate::test_utils::init_test_tracing();
    super::with_test_heap(|| {
        let mut x = super::Value::list(vec![super::Value::fixnum(1)]);
        for _ in 0..64 {
            x = super::Value::list(vec![x, x]);
        }
        let mut set = std::collections::HashSet::new();
        assert!(set.insert(x));
        assert!(set.contains(&x));
        let twin = super::Value::list(vec![super::Value::symbol("a"), x]);
        let other = super::Value::list(vec![super::Value::symbol("a"), x]);
        assert!(set.insert(twin));
        assert!(!set.insert(other), "an `equal` twin must hash alike");
        assert_eq!(set.len(), 2);
    });
}

/// A hash-table literal files `equal` keys once, however large: two equal
/// 256-element vectors or 300-element lists are one entry, the later value
/// winning, as GNU's reader gives.
#[test]
fn equal_hash_literals_merge_equal_keys_beyond_the_key_budget() {
    crate::test_utils::init_test_tracing();
    let form = r##"
(list
 (let* ((v (make-vector 256 0))
        (h (read (format "#s(hash-table test equal data (%S a %S b))" v v))))
   (list (hash-table-count h) (gethash v h) (progn (remhash v h) (hash-table-count h))))
 (let* ((l (make-list 300 'x))
        (h (read (format "#s(hash-table test equal data (%S a %S b))" l l))))
   (list (hash-table-count h) (gethash l h) (progn (remhash l h) (hash-table-count h)))))
"##;
    assert_eq!(
        runtime_startup_eval_one(form),
        oracle_expect_transcript(r#""OK ((1 b 0) (1 b 0))""#)
    );
}
