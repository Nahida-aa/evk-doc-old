use std::{
    borrow::Cow,
    cmp::{self, Ordering},
    ops::{AddAssign, Range, RangeInclusive},
};
pub mod paths;
pub mod redact;
pub mod rel_path;
pub mod schemars;
pub mod serde;
pub mod shell;
pub use gpui_util::*;

pub fn post_inc<T: From<u8> + AddAssign<T> + Copy>(value: &mut T) -> T {
    let prev = *value;
    *value += T::from(1);
    prev
}

pub trait RangeExt<T> {
    fn sorted(&self) -> Self;
    fn to_inclusive(&self) -> RangeInclusive<T>;
    fn overlaps(&self, other: &Range<T>) -> bool;
    fn contains_inclusive(&self, other: &Range<T>) -> bool;
}
impl<T: Ord + Clone> RangeExt<T> for Range<T> {
    fn sorted(&self) -> Self {
        cmp::min(&self.start, &self.end).clone()..cmp::max(&self.start, &self.end).clone()
    }

    fn to_inclusive(&self) -> RangeInclusive<T> {
        self.start.clone()..=self.end.clone()
    }

    fn overlaps(&self, other: &Range<T>) -> bool {
        self.start < other.end && other.start < self.end
    }

    fn contains_inclusive(&self, other: &Range<T>) -> bool {
        self.start <= other.start && other.end <= self.end
    }
}
impl<T: Ord + Clone> RangeExt<T> for RangeInclusive<T> {
    fn sorted(&self) -> Self {
        cmp::min(self.start(), self.end()).clone()..=cmp::max(self.start(), self.end()).clone()
    }

    fn to_inclusive(&self) -> RangeInclusive<T> {
        self.clone()
    }

    fn overlaps(&self, other: &Range<T>) -> bool {
        self.start() < &other.end && &other.start <= self.end()
    }

    fn contains_inclusive(&self, other: &Range<T>) -> bool {
        self.start() <= &other.start && &other.end <= self.end()
    }
}

/// Removes characters from the front of the string if its length is greater than `max_chars` and
/// prepends the string with "...". Returns string unchanged if its length is smaller than max_chars.
pub fn truncate_and_remove_front(s: &str, max_chars: usize) -> String {
    debug_assert!(max_chars >= 5);

    // If the string's byte length is <= max_chars, walking the string can be skipped since the
    // number of chars is <= the number of bytes.
    if s.len() <= max_chars {
        return s.to_string();
    }
    let suffix_char_length = max_chars.saturating_sub(1);
    let truncation_ix = s
        .char_indices()
        .map(|(i, _)| i)
        .nth_back(suffix_char_length);
    match truncation_ix {
        Some(index) if index > 0 => "…".to_string() + &s[index..],
        _ => s.to_string(),
    }
}

/// Expands to an immediately-invoked function expression. Good for using the ? operator
/// in functions which do not return an Option or Result.
///
/// Accepts a normal block, an async block, or an async move block.
#[macro_export]
macro_rules! maybe {
    ($block:block) => {
        (|| $block)()
    };
    (async $block:block) => {
        (async || $block)()
    };
    (async move $block:block) => {
        (async move || $block)()
    };
}

/// Removes characters from the end of the string if its length is greater than `max_chars` and
/// appends "..." to the string. Returns string unchanged if its length is smaller than max_chars.
pub fn truncate_and_trailoff(s: &str, max_chars: usize) -> String {
    debug_assert!(max_chars >= 5);

    // If the string's byte length is <= max_chars, walking the string can be skipped since the
    // number of chars is <= the number of bytes.
    if s.len() <= max_chars {
        return s.to_string();
    }
    let truncation_ix = s.char_indices().map(|(i, _)| i).nth(max_chars);
    match truncation_ix {
        Some(index) => s[..index].to_string() + "…",
        _ => s.to_string(),
    }
}
