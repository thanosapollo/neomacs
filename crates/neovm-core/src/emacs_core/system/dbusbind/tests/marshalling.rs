//! Outgoing marshalling must reject Lisp input before entering libdbus containers.

use super::super::types::{append_arg, object_to_arg_type};
use crate::emacs_core::value::Value;
use dbus::Message;
use dbus::arg::{ArgType, IterAppend};

fn kw(name: &str) -> Value {
    Value::keyword_by_name(name)
}
fn list(values: Vec<Value>) -> Value {
    Value::list(values)
}
fn message() -> Message {
    Message::new_signal("/org/neomacs/Test", "org.neomacs.Test", "Notify").unwrap()
}
fn marshal(value: Value) -> Message {
    let mut message = message();
    append_arg(
        &mut IterAppend::new(&mut message),
        object_to_arg_type(value).unwrap(),
        value,
    )
    .unwrap();
    message
}
fn signature(value: Value) -> String {
    marshal(value).iter_init().signature().to_string()
}
fn variant(value: Value) -> Value {
    list(vec![kw(":variant"), value])
}
fn entry(key: &str, value: Value) -> Value {
    list(vec![kw(":dict-entry"), Value::string(key), variant(value)])
}
fn hints() -> Value {
    list(vec![
        kw(":array"),
        entry("urgency", Value::fixnum(1)),
        entry("category", Value::string("im.received")),
        entry(
            "image-data",
            list(vec![
                kw(":struct"),
                Value::fixnum(1),
                Value::fixnum(1),
                Value::fixnum(4),
                Value::T,
                Value::fixnum(8),
                Value::fixnum(4),
                list(vec![kw(":array"), kw(":byte"), Value::fixnum(255)]),
            ]),
        ),
    ])
}

#[test]
fn notification_dictionary_has_recursive_signature_and_values() {
    let _ctx = crate::emacs_core::eval::Context::new();
    let msg = marshal(hints());
    assert_eq!(msg.iter_init().signature().to_string(), "a{sv}");
    let dict: std::collections::HashMap<String, dbus::arg::Variant<Box<dyn dbus::arg::RefArg>>> =
        msg.read1().unwrap();
    assert_eq!(dict.len(), 3);
    assert_eq!(dict["urgency"].0.as_u64(), Some(1));
    assert_eq!(dict["category"].0.as_str(), Some("im.received"));
    assert_eq!(dict["image-data"].0.signature().to_string(), "(uuubuuay)");
}

#[test]
fn arrays_variants_and_structs_keep_full_signatures() {
    let _ctx = crate::emacs_core::eval::Context::new();
    assert_eq!(
        signature(list(vec![
            kw(":array"),
            list(vec![kw(":array"), Value::string("a")]),
            list(vec![kw(":array"), Value::string("b")])
        ])),
        "aas"
    );
    assert_eq!(
        signature(list(vec![
            kw(":array"),
            list(vec![kw(":struct"), Value::string("a"), Value::fixnum(1)])
        ])),
        "a(su)"
    );
    assert_eq!(signature(variant(hints())), "v");
    assert_eq!(signature(variant(variant(Value::string("nested")))), "v");
    assert_eq!(
        signature(list(vec![
            kw(":array"),
            variant(Value::string("a")),
            variant(Value::fixnum(1))
        ])),
        "av"
    );
}

#[test]
fn empty_arrays_preserve_gnu_explicit_element_signatures() {
    let _ctx = crate::emacs_core::eval::Context::new();
    for (value, expected) in [
        (list(vec![kw(":array")]), "as"),
        (
            list(vec![kw(":array"), kw(":signature"), Value::string("{sv}")]),
            "a{sv}",
        ),
        (
            list(vec![kw(":array"), kw(":signature"), Value::string("(su)")]),
            "a(su)",
        ),
    ] {
        let msg = marshal(value);
        assert_eq!(msg.iter_init().signature().to_string(), expected);
        let mut iter = msg.iter_init().recurse(ArgType::Array).unwrap();
        assert_eq!(iter.arg_type(), ArgType::Invalid);
    }
}

#[test]
fn malformed_containers_return_lisp_errors_without_partial_append() {
    let _ctx = crate::emacs_core::eval::Context::new();
    let cases = vec![
        list(vec![kw(":array"), Value::string("a"), Value::fixnum(1)]),
        list(vec![
            kw(":array"),
            list(vec![kw(":struct"), Value::string("a")]),
            list(vec![kw(":struct"), Value::fixnum(1)]),
        ]),
        list(vec![kw(":variant")]),
        list(vec![kw(":variant"), Value::fixnum(1), Value::fixnum(2)]),
        list(vec![kw(":struct")]),
        entry("outside-array", Value::T),
        list(vec![
            kw(":array"),
            list(vec![kw(":dict-entry"), Value::string("missing-value")]),
        ]),
        list(vec![
            kw(":array"),
            list(vec![kw(":dict-entry"), variant(Value::T), Value::T]),
        ]),
        list(vec![kw(":array"), kw(":signature"), Value::string("ss")]),
        list(vec![kw(":array"), kw(":signature"), Value::string("a")]),
        list(vec![kw(":array"), kw(":byte"), Value::fixnum(256)]),
        list(vec![
            kw(":struct"),
            Value::string("first"),
            kw(":uint32"),
            Value::string("not-integer"),
        ]),
        list(vec![kw(":struct"), kw(":uint32")]),
        Value::cons(kw(":array"), Value::fixnum(2)),
    ];
    for (index, value) in cases.into_iter().enumerate() {
        let mut msg = message();
        assert!(
            append_arg(
                &mut IterAppend::new(&mut msg),
                object_to_arg_type(value).unwrap(),
                value
            )
            .is_err(),
            "case {index}"
        );
        assert_eq!(
            msg.iter_init().arg_type(),
            ArgType::Invalid,
            "partial case {index}"
        );
        append_arg(
            &mut IterAppend::new(&mut msg),
            ArgType::String,
            Value::string("still alive"),
        )
        .unwrap();
        assert_eq!(msg.read1::<String>().unwrap(), "still alive");
    }
}

#[test]
fn invalid_paths_signatures_and_nuls_are_lisp_errors() {
    let _ctx = crate::emacs_core::eval::Context::new();
    for (kind, value) in [
        (ArgType::ObjectPath, "not/a/path"),
        (ArgType::Signature, "a"),
        (ArgType::String, "bad\0string"),
        (ArgType::ObjectPath, "/ok\0bad"),
        (ArgType::Signature, "s\0bad"),
    ] {
        let mut msg = message();
        assert!(append_arg(&mut IterAppend::new(&mut msg), kind, Value::string(value)).is_err());
        assert_eq!(msg.iter_init().arg_type(), ArgType::Invalid);
    }
}

#[test]
fn excessive_nesting_and_circular_lists_are_lisp_errors() {
    let _ctx = crate::emacs_core::eval::Context::new();
    let mut nested = Value::string("leaf");
    for _ in 0..70 {
        nested = variant(nested);
    }
    let mut msg = message();
    assert!(append_arg(&mut IterAppend::new(&mut msg), ArgType::Variant, nested).is_err());
    let cycle = list(vec![kw(":array"), Value::string("cycle")]);
    cycle.set_cdr(cycle);
    assert!(append_arg(&mut IterAppend::new(&mut msg), ArgType::Array, cycle).is_err());
    let mut fields = vec![kw(":struct")];
    fields.extend(std::iter::repeat_n(Value::fixnum(1), 256));
    assert!(
        append_arg(
            &mut IterAppend::new(&mut msg),
            ArgType::Struct,
            list(fields)
        )
        .is_err()
    );
}

/// Opt-in transport check: run ONLY under a disposable dbus-run-session, with
/// NEOMACS_DBUS_PRIVATE_TEST=1. Ordinary unit tests never open the user's bus.
#[test]
#[ignore = "requires explicitly selected disposable dbus-run-session"]
fn private_bus_notification_composites_survive_rejected_arguments() {
    use dbus::channel::{BusType, Channel};
    use std::time::{Duration, Instant};
    assert_eq!(
        std::env::var("NEOMACS_DBUS_PRIVATE_TEST").as_deref(),
        Ok("1")
    );
    let _ctx = crate::emacs_core::eval::Context::new();
    let receiver = Channel::get_private(BusType::Session).unwrap();
    let sender = Channel::get_private(BusType::Session).unwrap();
    for round in 0..2 {
        let mut msg = Message::new_method_call(
            receiver.unique_name().unwrap(),
            "/org/freedesktop/Notifications",
            "org.freedesktop.Notifications",
            "Notify",
        )
        .unwrap();
        let mut iter = IterAppend::new(&mut msg);
        for (kind, value) in [
            (ArgType::String, Value::string("Neomacs test")),
            (ArgType::UInt32, Value::fixnum(0)),
            (ArgType::String, Value::string("")),
            (ArgType::String, Value::string("Test message")),
            (ArgType::String, Value::string("body")),
            (ArgType::Array, list(vec![kw(":array")])),
            (ArgType::Array, hints()),
            (ArgType::Int32, Value::fixnum(-1)),
        ] {
            append_arg(&mut iter, kind, value).unwrap();
        }
        let serial = sender.send(msg).unwrap();
        sender.flush();
        let deadline = Instant::now() + Duration::from_secs(3);
        let received = loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .expect("Notify delivery timed out");
            let msg = receiver
                .blocking_pop_message(remaining)
                .unwrap()
                .expect("Notify delivery timed out");
            if msg.member().as_deref() == Some("Notify") {
                break msg;
            }
        };
        assert_eq!(received.get_serial(), Some(serial));
        let signatures: String = received
            .iter_init()
            .map(|arg| arg.signature().to_string())
            .collect();
        assert_eq!(signatures, "susssasa{sv}i");
        let mut body = received.iter_init();
        for _ in 0..6 {
            assert!(body.next());
        }
        let dict = body.get::<std::collections::HashMap<String, dbus::arg::Variant<Box<dyn dbus::arg::RefArg>>>>().unwrap();
        assert_eq!(dict["urgency"].0.as_u64(), Some(1));
        assert_eq!(dict["image-data"].0.signature().to_string(), "(uuubuuay)");
        if round == 0 {
            let mut rejected = message();
            let bad = list(vec![
                kw(":struct"),
                Value::string("first"),
                kw(":byte"),
                Value::fixnum(999),
            ]);
            assert!(append_arg(&mut IterAppend::new(&mut rejected), ArgType::Struct, bad).is_err());
            assert_eq!(rejected.iter_init().arg_type(), ArgType::Invalid);
        }
    }
}
