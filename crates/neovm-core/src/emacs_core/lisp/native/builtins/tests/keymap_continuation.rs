use super::plan_keymap_iteration;
use crate::emacs_core::value::Value;

fn map(entries: Vec<Value>) -> Value {
    Value::cons(Value::symbol("keymap"), Value::list(entries))
}

#[test]
fn composed_continuation_is_exact_remaining_spine() {
    let _ctx = crate::emacs_core::eval::Context::new();
    let first = map(vec![]);
    let second = map(vec![]);
    let composed = map(vec![first, second]);
    let plan = plan_keymap_iteration(composed);
    assert_eq!(plan.parent, composed.cons_cdr());
    assert_eq!(plan.parent.cons_car(), first);
    assert_eq!(plan.parent.cons_cdr().cons_car(), second);
    assert!(plan.bindings.is_empty());
}

#[test]
fn nested_composition_is_not_flattened_or_consumed() {
    let _ctx = crate::emacs_core::eval::Context::new();
    let first = map(vec![]);
    let second = map(vec![]);
    let nested = map(vec![first, second]);
    let last = map(vec![]);
    let composed = map(vec![nested, last]);
    let plan = plan_keymap_iteration(composed);
    assert_eq!(plan.parent, composed.cons_cdr());
    assert_eq!(plan.parent.cons_car(), nested);
    assert_eq!(plan.parent.cons_cdr().cons_car(), last);
    let inner = plan_keymap_iteration(nested);
    assert_eq!(inner.parent, nested.cons_cdr());
    assert!(plan.bindings.is_empty());
}

#[test]
fn local_bindings_keep_order_before_components_and_parent() {
    let _ctx = crate::emacs_core::eval::Context::new();
    let first = map(vec![]);
    let second = map(vec![]);
    let parent = map(vec![]);
    let continuation = Value::cons(first, Value::cons(second, parent));
    let a = (Value::fixnum(97), Value::symbol("local-a"));
    let b = (Value::fixnum(98), Value::symbol("local-b"));
    let composed = Value::cons(
        Value::symbol("keymap"),
        Value::cons(
            Value::cons(a.0, a.1),
            Value::cons(Value::cons(b.0, b.1), continuation),
        ),
    );
    let plan = plan_keymap_iteration(composed);
    assert_eq!(plan.bindings, vec![a, b]);
    assert_eq!(plan.parent, continuation);
    assert_eq!(plan.parent.cons_cdr().cons_cdr(), parent);
}

#[test]
fn ordinary_parent_and_empty_map_are_unchanged() {
    let _ctx = crate::emacs_core::eval::Context::new();
    let parent = map(vec![]);
    let binding = (Value::fixnum(97), Value::symbol("local"));
    let child = Value::cons(
        Value::symbol("keymap"),
        Value::cons(Value::cons(binding.0, binding.1), parent),
    );
    let plan = plan_keymap_iteration(child);
    assert_eq!(plan.bindings, vec![binding]);
    assert_eq!(plan.parent, parent);
    let empty = plan_keymap_iteration(map(vec![]));
    assert!(empty.bindings.is_empty());
    assert!(empty.parent.is_nil());
}
