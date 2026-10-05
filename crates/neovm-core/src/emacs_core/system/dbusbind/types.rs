//! GNU Lisp ↔ D-Bus type conversion (`xd_symbol_to_dbus_type`, `xd_append_arg`,
//! `xd_retrieve_arg` in `src/dbusbind.c`).

use dbus::arg::messageitem::{MessageItem, MessageItemArray, MessageItemDict};
use dbus::arg::{ArgType, Iter, IterAppend};
use dbus::strings::{Path as DbusPath, Signature};

use crate::emacs_core::error::{Flow, LispCondition, signal};
use crate::emacs_core::value::Value;

use super::connection::dbus_error;

pub(super) fn keyword_for(arg_type: ArgType) -> Value {
    Value::keyword_by_name(match arg_type {
        ArgType::Byte => ":byte",
        ArgType::Boolean => ":boolean",
        ArgType::Int16 => ":int16",
        ArgType::UInt16 => ":uint16",
        ArgType::Int32 => ":int32",
        ArgType::UInt32 => ":uint32",
        ArgType::Int64 => ":int64",
        ArgType::UInt64 => ":uint64",
        ArgType::Double => ":double",
        ArgType::String => ":string",
        ArgType::ObjectPath => ":object-path",
        ArgType::Signature => ":signature",
        ArgType::UnixFd => ":unix-fd",
        ArgType::Array => ":array",
        ArgType::Variant => ":variant",
        ArgType::Struct => ":struct",
        ArgType::DictEntry => ":dict-entry",
        ArgType::Invalid => ":invalid",
    })
}

pub(super) fn is_type_keyword(value: Value) -> bool {
    symbol_to_arg_type(value).is_some()
}

pub(super) fn symbol_to_arg_type(value: Value) -> Option<ArgType> {
    if !value.is_keyword() {
        return None;
    }
    if value == Value::keyword_by_name(":byte") {
        Some(ArgType::Byte)
    } else if value == Value::keyword_by_name(":boolean") {
        Some(ArgType::Boolean)
    } else if value == Value::keyword_by_name(":int16") {
        Some(ArgType::Int16)
    } else if value == Value::keyword_by_name(":uint16") {
        Some(ArgType::UInt16)
    } else if value == Value::keyword_by_name(":int32") {
        Some(ArgType::Int32)
    } else if value == Value::keyword_by_name(":uint32") {
        Some(ArgType::UInt32)
    } else if value == Value::keyword_by_name(":int64") {
        Some(ArgType::Int64)
    } else if value == Value::keyword_by_name(":uint64") {
        Some(ArgType::UInt64)
    } else if value == Value::keyword_by_name(":double") {
        Some(ArgType::Double)
    } else if value == Value::keyword_by_name(":string") {
        Some(ArgType::String)
    } else if value == Value::keyword_by_name(":object-path") {
        Some(ArgType::ObjectPath)
    } else if value == Value::keyword_by_name(":signature") {
        Some(ArgType::Signature)
    } else if value == Value::keyword_by_name(":unix-fd") {
        Some(ArgType::UnixFd)
    } else if value == Value::keyword_by_name(":array") {
        Some(ArgType::Array)
    } else if value == Value::keyword_by_name(":variant") {
        Some(ArgType::Variant)
    } else if value == Value::keyword_by_name(":struct") {
        Some(ArgType::Struct)
    } else if value == Value::keyword_by_name(":dict-entry") {
        Some(ArgType::DictEntry)
    } else {
        None
    }
}

pub(super) fn is_basic(arg_type: ArgType) -> bool {
    !matches!(
        arg_type,
        ArgType::Array | ArgType::Variant | ArgType::Struct | ArgType::DictEntry | ArgType::Invalid
    )
}

/// GNU `XD_OBJECT_TO_DBUS_TYPE`.
pub(super) fn object_to_arg_type(object: Value) -> Result<ArgType, Flow> {
    if object == Value::T || object.is_nil() {
        return Ok(ArgType::Boolean);
    }
    if let Some(n) = object.as_fixnum() {
        return Ok(if n >= 0 {
            ArgType::UInt32
        } else {
            ArgType::Int32
        });
    }
    if object.is_float() {
        return Ok(ArgType::Double);
    }
    if object.is_string() {
        return Ok(ArgType::String);
    }
    if let Some(arg_type) = symbol_to_arg_type(object) {
        return Ok(arg_type);
    }
    if object.is_cons() {
        let car = object.cons_car();
        if let Some(inner) = symbol_to_arg_type(car) {
            return Ok(if is_basic(inner) {
                ArgType::Array
            } else {
                inner
            });
        }
        return Ok(ArgType::Array);
    }
    Err(dbus_error("Unable to determine D-Bus type"))
}

pub(super) fn append_arg(
    iter: &mut IterAppend<'_>,
    arg_type: ArgType,
    object: Value,
) -> Result<(), Flow> {
    // IterAppend callbacks cannot return Flow, and closing a partially written
    // container can abort in libdbus. Validate and own the entire argument first.
    // MessageItem derives recursive wire signatures, not the ArgType enum tag
    // (notably 'r'/'e', which are not signature characters).
    iter.append(prepare_arg(arg_type, object, 0)?);
    Ok(())
}

fn checked_signature(signature: String) -> Result<Signature<'static>, Flow> {
    Signature::new(signature).map_err(|err| dbus_error(&err))
}

fn prepare_arg(arg_type: ArgType, object: Value, depth: usize) -> Result<MessageItem, Flow> {
    // Variants hide their contents in the outer signature. Bound traversal as
    // well as validating signatures, including recursively self-containing Lisp.
    if depth >= 64 {
        return Err(dbus_error("D-Bus container nesting is too deep"));
    }
    Ok(match arg_type {
        ArgType::Boolean => {
            if object == keyword_for(ArgType::Boolean) {
                return Err(dbus_error("Missing D-Bus boolean value"));
            }
            MessageItem::Bool(object.is_truthy())
        }
        ArgType::Byte => MessageItem::Byte(unsigned(object, u8::MAX as u64)? as u8),
        ArgType::Int16 => {
            MessageItem::Int16(signed(object, i16::MIN as i64, i16::MAX as i64)? as i16)
        }
        ArgType::UInt16 => MessageItem::UInt16(unsigned(object, u16::MAX as u64)? as u16),
        ArgType::Int32 => {
            MessageItem::Int32(signed(object, i32::MIN as i64, i32::MAX as i64)? as i32)
        }
        ArgType::UInt32 | ArgType::UnixFd => {
            MessageItem::UInt32(unsigned(object, u32::MAX as u64)? as u32)
        }
        ArgType::Int64 => MessageItem::Int64(signed(object, i64::MIN, i64::MAX)?),
        ArgType::UInt64 => MessageItem::UInt64(unsigned(object, u64::MAX)?),
        ArgType::Double => MessageItem::Double(object.as_float().ok_or_else(|| {
            signal(
                LispCondition::WrongTypeArgument,
                vec![Value::symbol("numberp"), object],
            )
        })?),
        ArgType::String => MessageItem::Str(string_arg(object)?),
        ArgType::ObjectPath => MessageItem::ObjectPath(
            DbusPath::new(string_arg(object)?).map_err(|err| dbus_error(&err))?,
        ),
        ArgType::Signature => MessageItem::Signature(checked_signature(string_arg(object)?)?),
        ArgType::Array => prepare_array(container_elements(arg_type, object)?, depth + 1)?,
        ArgType::Variant | ArgType::Struct => {
            let elements = container_elements(arg_type, object)?;
            let mut items = elements
                .into_iter()
                .map(|(kind, value)| prepare_arg(kind, value, depth + 1))
                .collect::<Result<Vec<_>, _>>()?;
            if arg_type == ArgType::Variant {
                if items.len() != 1 {
                    return Err(dbus_error("D-Bus variant must contain exactly one value"));
                }
                MessageItem::Variant(Box::new(items.remove(0)))
            } else {
                // MessageItem::signature uses an infallible constructor for
                // structs; establish its nonempty/length/depth invariant here.
                checked_signature(format!("({})", item_signatures(&items)))?;
                MessageItem::Struct(items)
            }
        }
        ArgType::DictEntry => return Err(dbus_error("D-Bus dictionary entry outside an array")),
        ArgType::Invalid => return Err(dbus_error("Invalid D-Bus type")),
    })
}

/// GNU's optional array marker and alternating TYPE VALUE element syntax.
/// Other containers require their marker; dotted and cyclic lists are errors.
fn container_elements(arg_type: ArgType, object: Value) -> Result<Vec<(ArgType, Value)>, Flow> {
    let values = crate::emacs_core::value::list_to_vec(&object)
        .ok_or_else(|| dbus_error("D-Bus container must be a proper, non-circular list"))?;
    let has_marker = values.first().copied() == Some(keyword_for(arg_type));
    if arg_type != ArgType::Array && !has_marker {
        return Err(dbus_error("Missing D-Bus container type"));
    }
    let mut index = usize::from(has_marker);
    let mut elements = Vec::new();
    while index < values.len() {
        let kind = object_to_arg_type(values[index])?;
        if is_type_keyword(values[index]) {
            index += 1;
        }
        let value = values
            .get(index)
            .copied()
            .ok_or_else(|| dbus_error("Missing D-Bus argument value"))?;
        elements.push((kind, value));
        index += 1;
    }
    Ok(elements)
}

fn item_signatures(items: &[MessageItem]) -> String {
    items
        .iter()
        .map(|item| item.signature().to_string())
        .collect()
}

fn prepare_array(elements: Vec<(ArgType, Value)>, depth: usize) -> Result<MessageItem, Flow> {
    // GNU (:array :signature "{sv}") denotes an EMPTY array, not an array
    // containing a signature value. Validate in array context: {sv} alone
    // cannot be constructed as a dbus::Signature.
    if let [(ArgType::Signature, value)] = elements.as_slice() {
        let signature = checked_signature(format!("a{}", string_arg(*value)?))?;
        return MessageItemArray::new(Vec::new(), signature)
            .map(MessageItem::Array)
            .map_err(|_| dbus_error("Invalid D-Bus array signature"));
    }
    if elements
        .first()
        .is_some_and(|(kind, _)| *kind == ArgType::DictEntry)
    {
        let mut entries = Vec::new();
        for (kind, value) in elements {
            if kind != ArgType::DictEntry {
                return Err(dbus_error("Inconsistent D-Bus array element types"));
            }
            let fields = container_elements(kind, value)?;
            let [(key_type, key), (value_type, value)] = fields.as_slice() else {
                return Err(dbus_error("D-Bus dictionary entry must contain two values"));
            };
            if !is_basic(*key_type) {
                return Err(dbus_error("D-Bus dictionary key must be basic"));
            }
            entries.push((
                prepare_arg(*key_type, *key, depth + 1)?,
                prepare_arg(*value_type, *value, depth + 1)?,
            ));
        }
        let (key, value) = &entries[0]; // nonempty by the first-element check
        let key_signature = key.signature();
        let value_signature = value.signature();
        // MessageItemDict::new uses Signature::from internally.
        checked_signature(format!("a{{{}{}}}", key_signature, value_signature))?;
        return MessageItemDict::new(entries, key_signature, value_signature)
            .map(MessageItem::Dict)
            .map_err(|_| dbus_error("Inconsistent D-Bus dictionary element signatures"));
    }
    let items = elements
        .into_iter()
        .map(|(kind, value)| prepare_arg(kind, value, depth))
        .collect::<Result<Vec<_>, _>>()?;
    let element_signature = items
        .first()
        .map(|item| item.signature().to_string())
        .unwrap_or_else(|| "s".to_owned());
    let signature = checked_signature(format!("a{element_signature}"))?;
    MessageItemArray::new(items, signature)
        .map(MessageItem::Array)
        .map_err(|_| dbus_error("Inconsistent D-Bus array element signatures"))
}

pub(super) fn retrieve_arg(iter: &mut Iter<'_>) -> Result<Value, Flow> {
    let arg_type = iter.arg_type();
    let typed = match arg_type {
        ArgType::Byte => Value::fixnum(iter.get::<u8>().unwrap_or(0) as i64),
        ArgType::Boolean => Value::bool_val(iter.get::<bool>().unwrap_or(false)),
        ArgType::Int16 => Value::fixnum(iter.get::<i16>().unwrap_or(0) as i64),
        ArgType::UInt16 => Value::fixnum(iter.get::<u16>().unwrap_or(0) as i64),
        ArgType::Int32 => Value::fixnum(iter.get::<i32>().unwrap_or(0) as i64),
        ArgType::UInt32 | ArgType::UnixFd => Value::fixnum(iter.get::<u32>().unwrap_or(0) as i64),
        ArgType::Int64 => Value::fixnum(iter.get::<i64>().unwrap_or(0)),
        ArgType::UInt64 => {
            let n = iter.get::<u64>().unwrap_or(0);
            if n <= i64::MAX as u64 {
                Value::fixnum(n as i64)
            } else {
                Value::string(n.to_string())
            }
        }
        ArgType::Double => Value::make_float(iter.get::<f64>().unwrap_or(0.0)),
        ArgType::String => Value::string(iter.get::<String>().unwrap_or_default()),
        ArgType::ObjectPath => Value::string(
            iter.get::<DbusPath<'_>>()
                .map(|path| path.to_string())
                .unwrap_or_default(),
        ),
        ArgType::Signature => Value::string(
            iter.get::<Signature<'_>>()
                .map(|signature| signature.to_string())
                .unwrap_or_default(),
        ),
        ArgType::Array | ArgType::Variant | ArgType::Struct | ArgType::DictEntry => {
            let mut inner = iter
                .recurse(arg_type)
                .ok_or_else(|| dbus_error("Cannot read container"))?;
            let mut items = Vec::new();
            while inner.arg_type() != ArgType::Invalid {
                items.push(retrieve_arg(&mut inner)?);
                let _ = Iter::next(&mut inner);
            }
            return Ok(Value::cons(keyword_for(arg_type), Value::list(items)));
        }
        ArgType::Invalid => return Ok(Value::NIL),
    };
    Ok(Value::list(vec![keyword_for(arg_type), typed]))
}

fn string_arg(value: Value) -> Result<String, Flow> {
    let text = value.as_utf8_str().ok_or_else(|| {
        signal(
            LispCondition::WrongTypeArgument,
            vec![Value::symbol("stringp"), value],
        )
    })?;
    if text.contains('\0') {
        return Err(dbus_error("D-Bus strings cannot contain NUL"));
    }
    Ok(text.to_owned())
}

fn signed(value: Value, min: i64, max: i64) -> Result<i64, Flow> {
    let n = value.as_fixnum().ok_or_else(|| {
        signal(
            LispCondition::WrongTypeArgument,
            vec![Value::symbol("integerp"), value],
        )
    })?;
    if n < min || n > max {
        return Err(dbus_error("Integer out of range"));
    }
    Ok(n)
}

fn unsigned(value: Value, max: u64) -> Result<u64, Flow> {
    let n = value.as_fixnum().ok_or_else(|| {
        signal(
            LispCondition::WrongTypeArgument,
            vec![Value::symbol("integerp"), value],
        )
    })?;
    if n < 0 || n as u64 > max {
        return Err(dbus_error("Integer out of range"));
    }
    Ok(n as u64)
}
