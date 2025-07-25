// SPDX-License-Identifier: (Apache-2.0 OR MIT)

use crate::opt::{Opt, BIG_INTEGER, NAN_AS_NULL};

use crate::deserialize::pyobject::{
    get_unicode_key, parse_big_int, parse_f64, parse_false, parse_i64, parse_none, parse_true,
    parse_u64,
};
use crate::deserialize::DeserializeError;
use crate::ffi::yyjson::{
    yyjson_alc_pool_init, yyjson_doc, yyjson_read_err, yyjson_read_opts, yyjson_val,
    YYJSON_READ_SUCCESS,
};
use crate::str::PyStr;
use crate::util::usize_to_isize;
use core::ffi::c_char;
use core::ptr::{null, null_mut, NonNull};
use std::borrow::Cow;

const YYJSON_TAG_BIT: u8 = 8;

const YYJSON_VAL_SIZE: usize = core::mem::size_of::<yyjson_val>();

const TAG_RAW: u8 = 0b00000001;
const TAG_ARRAY: u8 = 0b00000110;
const TAG_DOUBLE: u8 = 0b00010100;
const TAG_FALSE: u8 = 0b00000011;
const TAG_INT64: u8 = 0b00001100;
const TAG_NULL: u8 = 0b00000010;
const TAG_OBJECT: u8 = 0b00000111;
const TAG_STRING: u8 = 0b00000101;
const TAG_TRUE: u8 = 0b00001011;
const TAG_UINT64: u8 = 0b00000100;

const YYJSON_READ_ALLOW_INF_AND_NAN: u32 = 1 << 4;
const YYJSON_READ_BIGNUM_AS_RAW: u32 = 1 << 7;

macro_rules! is_yyjson_tag {
    ($elem:expr, $tag:expr) => {
        unsafe { (*$elem).tag as u8 == $tag }
    };
}

fn yyjson_doc_get_root(doc: *mut yyjson_doc) -> *mut yyjson_val {
    unsafe { (*doc).root }
}

fn unsafe_yyjson_get_len(val: *mut yyjson_val) -> usize {
    unsafe { ((*val).tag >> YYJSON_TAG_BIT) as usize }
}

fn unsafe_yyjson_get_first(ctn: *mut yyjson_val) -> *mut yyjson_val {
    unsafe { ctn.add(1) }
}

const MINIMUM_BUFFER_CAPACITY: usize = 4096;

fn buffer_capacity_to_allocate(len: usize) -> usize {
    // The max memory size is (json_size / 2 * 16 * 1.5 + padding).
    (((len / 2) * 24) + 256 + (MINIMUM_BUFFER_CAPACITY - 1)) & !(MINIMUM_BUFFER_CAPACITY - 1)
}

fn unsafe_yyjson_is_ctn(val: *mut yyjson_val) -> bool {
    unsafe { (*val).tag as u8 & 0b00000110 == 0b00000110 }
}

#[allow(clippy::cast_ptr_alignment)]
fn unsafe_yyjson_get_next_container(val: *mut yyjson_val) -> *mut yyjson_val {
    unsafe { (val.cast::<u8>().add((*val).uni.ofs)).cast::<yyjson_val>() }
}

#[allow(clippy::cast_ptr_alignment)]
fn unsafe_yyjson_get_next_non_container(val: *mut yyjson_val) -> *mut yyjson_val {
    unsafe { (val.cast::<u8>().add(YYJSON_VAL_SIZE)).cast::<yyjson_val>() }
}

pub(crate) fn deserialize(
    data: &'static str,
    opts: Opt,
) -> Result<NonNull<pyo3_ffi::PyObject>, DeserializeError<'static>> {
    assume!(!data.is_empty());
    let buffer_capacity = buffer_capacity_to_allocate(data.len());
    let buffer_ptr = ffi!(PyMem_Malloc(buffer_capacity));
    if unlikely!(buffer_ptr.is_null()) {
        return Err(DeserializeError::from_yyjson(
            Cow::Borrowed("Not enough memory to allocate buffer for parsing"),
            0,
            data,
        ));
    }
    let mut alloc = crate::ffi::yyjson::yyjson_alc {
        malloc: None,
        realloc: None,
        free: None,
        ctx: null_mut(),
    };
    unsafe {
        yyjson_alc_pool_init(&raw mut alloc, buffer_ptr, buffer_capacity);
    }

    let mut err = yyjson_read_err {
        code: YYJSON_READ_SUCCESS,
        msg: null(),
        pos: 0,
    };

    let mut flag = 0;
    if opt_enabled!(opts, BIG_INTEGER) {
        flag |= YYJSON_READ_BIGNUM_AS_RAW
    }
    if opt_enabled!(opts, NAN_AS_NULL) {
        flag |= YYJSON_READ_ALLOW_INF_AND_NAN;
    }

    let doc = unsafe {
        yyjson_read_opts(
            data.as_ptr().cast::<c_char>().cast_mut(),
            data.len(),
            flag,
            &raw const alloc,
            &raw mut err,
        )
    };
    if unlikely!(doc.is_null()) {
        ffi!(PyMem_Free(buffer_ptr));
        let msg: Cow<str> = unsafe { core::ffi::CStr::from_ptr(err.msg).to_string_lossy() };
        return Err(DeserializeError::from_yyjson(msg, err.pos as i64, data));
    }
    let val = yyjson_doc_get_root(doc);
    let pyval = {
        if unlikely!(!unsafe_yyjson_is_ctn(val)) {
            let pyval = parse_element_non_container(val);
            pyval
        } else if is_yyjson_tag!(val, TAG_ARRAY) {
            let pyval = nonnull!(ffi!(PyList_New(usize_to_isize(unsafe_yyjson_get_len(val)))));
            if unsafe_yyjson_get_len(val) > 0 {
                populate_yy_array(pyval.as_ptr(), val);
            }
            pyval
        } else {
            let pyval = nonnull!(ffi!(_PyDict_NewPresized(usize_to_isize(
                unsafe_yyjson_get_len(val)
            ))));
            if unsafe_yyjson_get_len(val) > 0 {
                populate_yy_object(pyval.as_ptr(), val);
            }
            pyval
        }
    };
    ffi!(PyMem_Free(buffer_ptr));
    Ok(pyval)
}

enum ElementType {
    String,
    Uint64,
    Int64,
    Double,
    Null,
    True,
    False,
    Array,
    Object,
    Raw,
}

impl ElementType {
    fn from_tag(elem: *mut yyjson_val) -> Self {
        match unsafe { (*elem).tag as u8 } {
            TAG_STRING => Self::String,
            TAG_UINT64 => Self::Uint64,
            TAG_INT64 => Self::Int64,
            TAG_DOUBLE => Self::Double,
            TAG_NULL => Self::Null,
            TAG_TRUE => Self::True,
            TAG_FALSE => Self::False,
            TAG_ARRAY => Self::Array,
            TAG_OBJECT => Self::Object,
            TAG_RAW => Self::Raw,
            _ => unreachable_unchecked!(),
        }
    }
}

#[inline(always)]
fn parse_yy_string(elem: *mut yyjson_val) -> NonNull<pyo3_ffi::PyObject> {
    PyStr::from_str(str_from_slice!(
        (*elem).uni.str_.cast::<u8>(),
        unsafe_yyjson_get_len(elem)
    ))
    .as_non_null_ptr()
}

#[inline(always)]
fn parse_yy_raw(elem: *mut yyjson_val) -> NonNull<pyo3_ffi::PyObject> {
    let s = unsafe {
        std::slice::from_raw_parts((*elem).uni.str_.cast::<u8>(), unsafe_yyjson_get_len(elem))
    };
    // Only test the first 2 chars to be a valid JSON integer.
    // Accepts optional leading '-' and at least one digit, no other characters.
    if s.is_empty() {
        return parse_none();
    }
    let valid = match s {
        [b'-', d, ..] => d.is_ascii_digit(),
        [d, ..] => d.is_ascii_digit(),
        _ => false,
    };
    if !valid {
        return parse_none();
    }
    parse_big_int(unsafe { (*elem).uni.str_.cast::<std::os::raw::c_char>() })
}

#[inline(always)]
fn parse_yy_u64(elem: *mut yyjson_val) -> NonNull<pyo3_ffi::PyObject> {
    parse_u64(unsafe { (*elem).uni.u64_ })
}

#[inline(always)]
fn parse_yy_i64(elem: *mut yyjson_val) -> NonNull<pyo3_ffi::PyObject> {
    parse_i64(unsafe { (*elem).uni.i64_ })
}

#[inline(always)]
fn parse_yy_f64(elem: *mut yyjson_val) -> NonNull<pyo3_ffi::PyObject> {
    parse_f64(unsafe { (*elem).uni.f64_ })
}

macro_rules! append_to_list {
    ($dptr:expr, $pyval:expr) => {
        unsafe {
            core::ptr::write($dptr, $pyval);
            $dptr = $dptr.add(1);
        }
    };
}

#[inline(never)]
fn populate_yy_array(list: *mut pyo3_ffi::PyObject, elem: *mut yyjson_val) {
    unsafe {
        let len = unsafe_yyjson_get_len(elem);
        assume!(len >= 1);
        let mut next = unsafe_yyjson_get_first(elem);
        let mut dptr = (*list.cast::<pyo3_ffi::PyListObject>()).ob_item;

        for _ in 0..len {
            let val = next;
            if unlikely!(unsafe_yyjson_is_ctn(val)) {
                next = unsafe_yyjson_get_next_container(val);
                if is_yyjson_tag!(val, TAG_ARRAY) {
                    let pyval = ffi!(PyList_New(usize_to_isize(unsafe_yyjson_get_len(val))));
                    append_to_list!(dptr, pyval);
                    if unsafe_yyjson_get_len(val) > 0 {
                        populate_yy_array(pyval, val);
                    }
                } else {
                    let pyval = ffi!(_PyDict_NewPresized(usize_to_isize(unsafe_yyjson_get_len(
                        val
                    ))));
                    append_to_list!(dptr, pyval);
                    if unsafe_yyjson_get_len(val) > 0 {
                        populate_yy_object(pyval, val);
                    }
                }
            } else {
                next = unsafe_yyjson_get_next_non_container(val);
                let pyval = parse_element_non_container(val);
                append_to_list!(dptr, pyval.as_ptr());
            }
        }
    }
}

#[inline(never)]
fn populate_yy_object(dict: *mut pyo3_ffi::PyObject, elem: *mut yyjson_val) {
    unsafe {
        let len = unsafe_yyjson_get_len(elem);
        assume!(len >= 1);
        let mut next_key = unsafe_yyjson_get_first(elem);
        let mut next_val = next_key.add(1);
        for _ in 0..len {
            let val = next_val;
            let pykey = {
                let key_str = str_from_slice!(
                    (*next_key).uni.str_.cast::<u8>(),
                    unsafe_yyjson_get_len(next_key)
                );
                get_unicode_key(key_str)
            };
            if unlikely!(unsafe_yyjson_is_ctn(val)) {
                next_key = unsafe_yyjson_get_next_container(val);
                next_val = next_key.add(1);
                if is_yyjson_tag!(val, TAG_ARRAY) {
                    let pyval = ffi!(PyList_New(usize_to_isize(unsafe_yyjson_get_len(val))));
                    pydict_setitem!(dict, pykey.as_ptr(), pyval);
                    if unsafe_yyjson_get_len(val) > 0 {
                        populate_yy_array(pyval, val);
                    }
                } else {
                    let pyval = ffi!(_PyDict_NewPresized(usize_to_isize(unsafe_yyjson_get_len(
                        val
                    ))));
                    pydict_setitem!(dict, pykey.as_ptr(), pyval);
                    if unsafe_yyjson_get_len(val) > 0 {
                        populate_yy_object(pyval, val);
                    }
                }
            } else {
                next_key = unsafe_yyjson_get_next_non_container(val);
                next_val = next_key.add(1);
                let pyval = parse_element_non_container(val);
                pydict_setitem!(dict, pykey.as_ptr(), pyval.as_ptr());
            }
        }
    }
}

#[inline(always)]
fn parse_element_non_container(val: *mut yyjson_val) -> NonNull<pyo3_ffi::PyObject> {
    match ElementType::from_tag(val) {
        ElementType::Raw => parse_yy_raw(val),
        ElementType::String => parse_yy_string(val),
        ElementType::Uint64 => parse_yy_u64(val),
        ElementType::Int64 => parse_yy_i64(val),
        ElementType::Double => parse_yy_f64(val),
        ElementType::Null => parse_none(),
        ElementType::True => parse_true(),
        ElementType::False => parse_false(),
        ElementType::Array => unreachable_unchecked!(),
        ElementType::Object => unreachable_unchecked!(),
    }
}
