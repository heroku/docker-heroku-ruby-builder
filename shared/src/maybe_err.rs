//! Error-accumulation building blocks.
//!
//! Adapted from the [`proc_micro`](https://github.com/schneems/proc_micro) crate
//! and the blog post ["A daft proc-macro trick"][post], but with no dependency on
//! `syn`. The pattern lets a function *always* produce a value and *maybe* one or
//! more errors, so callers can accumulate as many problems as possible instead of
//! bailing on the first one.
//!
//! [post]: https://schneems.com/2025/03/26/a-daft-procmacro-trick-how-to-emit-partialcode-errors/
//!
//! Three cardinalities, three types:
//!
//! - [`MaybeFailures`] -- zero or more errors (the empty-able accumulator you push into).
//! - [`Failures`] -- one or more errors (the non-empty value you return).
//! - [`OkMaybe`] -- a value plus maybe one-or-more errors.
//!
//! The error behavior of a function is encoded in its return type:
//!
//! - Errors that block producing a value: `Result<T, Failures<E>>`.
//! - Errors that never block: `OkMaybe<T, Failures<E>>`.
//! - Errors that may or may not block: `Result<OkMaybe<T, Failures<E>>, Failures<E>>`.
//!   - `Ok(OkMaybe(value, None))` -- no errors.
//!   - `Ok(OkMaybe(value, Some(failures)))` -- error(s) that did not block.
//!   - `Err(failures)` -- could not produce a value due to error(s).

use std::fmt::{self, Display};
use std::num::NonZeroUsize;

/// One or more errors, guaranteed non-empty by construction.
///
/// This is the value you *return* when you have at least one error. It fills the
/// role `syn::Error` plays in `proc_micro`: a single value that always holds at
/// least one error and can hold many.
///
/// Build the first error with [`Failures::new`], then add more with
/// [`Failures::push`] or [`Extend`]. To collapse a possibly-empty pile of errors
/// into `Option<Failures<E>>`, accumulate into a [`MaybeFailures`] instead.
///
/// ```
/// use shared::maybe_err::Failures;
///
/// let mut failures = Failures::new("first".to_string());
/// failures.push("second".to_string());
///
/// assert_eq!(failures.len().get(), 2);
/// let collected: Vec<String> = failures.into_iter().collect();
/// assert_eq!(collected, vec!["first".to_string(), "second".to_string()]);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failures<E> {
    head: E,
    tail: Vec<E>,
}

impl<E> Failures<E> {
    /// Create a non-empty collection from a single error.
    ///
    /// ```
    /// use shared::maybe_err::Failures;
    ///
    /// let failures = Failures::new("boom".to_string());
    /// assert_eq!(failures.len().get(), 1);
    /// ```
    pub fn new(first: E) -> Self {
        Failures {
            head: first,
            tail: Vec::new(),
        }
    }

    /// Append another error.
    ///
    /// ```
    /// use shared::maybe_err::Failures;
    ///
    /// let mut failures = Failures::new("a".to_string());
    /// failures.push("b".to_string());
    /// assert_eq!(failures.len().get(), 2);
    /// ```
    pub fn push(&mut self, err: E) {
        self.tail.push(err);
    }

    /// The number of errors held, always at least one.
    ///
    /// Returning [`NonZeroUsize`] surfaces the non-empty guarantee in the type.
    ///
    /// ```
    /// use shared::maybe_err::Failures;
    ///
    /// let failures = Failures::new(());
    /// assert_eq!(failures.len().get(), 1);
    /// ```
    pub fn len(&self) -> NonZeroUsize {
        // SAFETY-equivalent: `1 + tail.len()` is always >= 1, so the unwrap cannot fail.
        NonZeroUsize::new(1 + self.tail.len()).expect("Failures always holds at least one error")
    }
}

impl<E> IntoIterator for Failures<E> {
    type Item = E;
    type IntoIter = std::iter::Chain<std::iter::Once<E>, std::vec::IntoIter<E>>;

    /// Iterate over every error, head first then the rest in push order.
    ///
    /// ```
    /// use shared::maybe_err::Failures;
    ///
    /// let mut failures = Failures::new(1);
    /// failures.push(2);
    /// failures.push(3);
    /// assert_eq!(failures.into_iter().collect::<Vec<_>>(), vec![1, 2, 3]);
    /// ```
    fn into_iter(self) -> Self::IntoIter {
        std::iter::once(self.head).chain(self.tail)
    }
}

impl<E> Extend<E> for Failures<E> {
    /// Append many errors at once. Also serves as a "combine": extend one
    /// `Failures` with the contents of another via its [`IntoIterator`].
    ///
    /// ```
    /// use shared::maybe_err::Failures;
    ///
    /// let mut a = Failures::new(1);
    /// let mut b = Failures::new(2);
    /// b.push(3);
    /// a.extend(b);
    /// assert_eq!(a.into_iter().collect::<Vec<_>>(), vec![1, 2, 3]);
    /// ```
    fn extend<I: IntoIterator<Item = E>>(&mut self, iter: I) {
        self.tail.extend(iter);
    }
}

impl<E: Display> Display for Failures<E> {
    /// A single error renders as that error; multiple render as a numbered block.
    ///
    /// ```
    /// use shared::maybe_err::Failures;
    ///
    /// let one = Failures::new("only".to_string());
    /// assert_eq!(one.to_string(), "only");
    ///
    /// let mut many = Failures::new("first".to_string());
    /// many.push("second".to_string());
    /// assert_eq!(many.to_string(), "2 errors:\n  1. first\n  2. second");
    /// ```
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let len = self.len();
        if len.get() == 1 {
            write!(f, "{}", self.head)
        } else {
            write!(f, "{} errors:", len)?;
            for (index, err) in
                std::iter::once(&self.head).chain(self.tail.iter()).enumerate()
            {
                write!(f, "\n  {}. {}", index + 1, err)?;
            }
            Ok(())
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for Failures<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.head)
    }
}

/// Zero or more errors: the empty-able accumulator you push into.
///
/// This fills the role of `proc_micro`'s `MaybeError`. It starts empty and, once
/// you are done accumulating, [`MaybeFailures::into_option`] collapses it into
/// `Option<Failures<E>>`: `None` means there were no errors, `Some` means one or
/// more.
///
/// ```
/// use shared::maybe_err::{Failures, MaybeFailures};
///
/// let mut errors = MaybeFailures::new();
/// assert!(errors.is_empty());
///
/// errors.push("nope".to_string());
/// errors.push("also nope".to_string());
///
/// let failures: Failures<String> = errors.into_option().expect("two errors");
/// assert_eq!(failures.len().get(), 2);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaybeFailures<E>(Option<Failures<E>>);

impl<E> MaybeFailures<E> {
    /// Create an empty accumulator.
    ///
    /// ```
    /// use shared::maybe_err::MaybeFailures;
    ///
    /// let errors: MaybeFailures<String> = MaybeFailures::new();
    /// assert!(errors.is_empty());
    /// ```
    pub fn new() -> Self {
        MaybeFailures(None)
    }

    /// Accumulate one error. The first push creates the underlying
    /// [`Failures`]; later pushes append to it.
    ///
    /// ```
    /// use shared::maybe_err::MaybeFailures;
    ///
    /// let mut errors = MaybeFailures::new();
    /// errors.push("boom".to_string());
    /// assert!(!errors.is_empty());
    /// ```
    pub fn push(&mut self, err: E) {
        match &mut self.0 {
            Some(failures) => failures.push(err),
            none => *none = Some(Failures::new(err)),
        }
    }

    /// Whether any error has been accumulated yet.
    ///
    /// ```
    /// use shared::maybe_err::MaybeFailures;
    ///
    /// let mut errors = MaybeFailures::new();
    /// assert!(errors.is_empty());
    /// errors.push(());
    /// assert!(!errors.is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.0.is_none()
    }

    /// Collapse into `Option<Failures<E>>`: `None` if empty, otherwise the
    /// accumulated non-empty [`Failures`].
    ///
    /// ```
    /// use shared::maybe_err::MaybeFailures;
    ///
    /// let empty: MaybeFailures<String> = MaybeFailures::new();
    /// assert!(empty.into_option().is_none());
    ///
    /// let mut errors = MaybeFailures::new();
    /// errors.push("boom".to_string());
    /// assert!(errors.into_option().is_some());
    /// ```
    pub fn into_option(self) -> Option<Failures<E>> {
        self.0
    }
}

impl<E> Default for MaybeFailures<E> {
    fn default() -> Self {
        MaybeFailures::new()
    }
}

impl<E> Extend<E> for MaybeFailures<E> {
    /// Accumulate many errors at once. This is what makes a `MaybeFailures` a
    /// valid [`OkMaybe::push_unwrap`] target.
    ///
    /// ```
    /// use shared::maybe_err::MaybeFailures;
    ///
    /// let mut errors = MaybeFailures::new();
    /// errors.extend(vec!["a".to_string(), "b".to_string()]);
    /// assert_eq!(errors.into_option().expect("two errors").len().get(), 2);
    /// ```
    fn extend<I: IntoIterator<Item = E>>(&mut self, iter: I) {
        for err in iter {
            self.push(err);
        }
    }
}

/// A value and maybe an error.
///
/// `OkMaybe(value, None)` carries a value with no error; `OkMaybe(value,
/// Some(err))` carries a value alongside an error. Because it is not a `Result`,
/// the `?` operator cannot accidentally discard the value via an early return,
/// which encourages accumulating errors rather than bailing on the first one.
///
/// The error type is generic. Pair it with [`Failures`] (`OkMaybe<T,
/// Failures<E>>`) when the error half should itself be able to hold one or more
/// errors.
///
/// ```
/// use shared::maybe_err::OkMaybe;
///
/// let ok: OkMaybe<i32, String> = OkMaybe(1, None);
/// assert_eq!(ok.to_result(), Ok(1));
///
/// let bad: OkMaybe<i32, String> = OkMaybe(2, Some("nope".to_string()));
/// assert_eq!(bad.to_result(), Err("nope".to_string()));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OkMaybe<T, E>(pub T, pub Option<E>);

impl<T, E> OkMaybe<T, E> {
    /// Convert into a `Result`: `Some` error becomes `Err`, `None` becomes
    /// `Ok(value)`. Use this when a partial value is not usable.
    ///
    /// ```
    /// use shared::maybe_err::OkMaybe;
    ///
    /// assert_eq!(OkMaybe::<_, String>((), None).to_result(), Ok(()));
    /// assert_eq!(OkMaybe((), Some("e".to_string())).to_result(), Err("e".to_string()));
    /// ```
    pub fn to_result(self) -> Result<T, E> {
        let OkMaybe(value, maybe) = self;
        match maybe {
            Some(err) => Err(err),
            None => Ok(value),
        }
    }

    /// Drain any error into an accumulator and return the value.
    ///
    /// The error type must be [`IntoIterator`] (as [`Failures`] is), so its
    /// errors can be pushed into any [`Extend`] target such as a
    /// [`MaybeFailures`] or a plain `Vec`. This is the key ergonomic for
    /// accumulating across many fallible steps in a loop.
    ///
    /// ```
    /// use shared::maybe_err::{Failures, MaybeFailures, OkMaybe};
    ///
    /// let mut errors: MaybeFailures<String> = MaybeFailures::new();
    ///
    /// let value = OkMaybe(10, Some(Failures::new("bad".to_string())))
    ///     .push_unwrap(&mut errors);
    ///
    /// assert_eq!(value, 10);
    /// assert!(!errors.is_empty());
    /// ```
    pub fn push_unwrap<G>(self, push_to: &mut impl Extend<G>) -> T
    where
        E: IntoIterator<Item = G>,
    {
        let OkMaybe(value, maybe) = self;
        if let Some(err) = maybe {
            push_to.extend(err);
        }
        value
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn failures_len_starts_at_one() {
        let failures = Failures::new("a");
        assert_eq!(failures.len().get(), 1);
    }

    #[test]
    fn failures_push_grows_len() {
        let mut failures = Failures::new("a");
        failures.push("b");
        failures.push("c");
        assert_eq!(failures.len().get(), 3);
    }

    #[test]
    fn failures_into_iter_is_head_then_tail() {
        let mut failures = Failures::new(1);
        failures.push(2);
        failures.push(3);
        assert_eq!(failures.into_iter().collect::<Vec<_>>(), vec![1, 2, 3]);
    }

    #[test]
    fn failures_extend_appends() {
        let mut failures = Failures::new(1);
        failures.extend(vec![2, 3]);

        let mut other = Failures::new(4);
        other.push(5);
        failures.extend(other);

        assert_eq!(failures.into_iter().collect::<Vec<_>>(), vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn failures_display_single() {
        let failures = Failures::new("only".to_string());
        assert_eq!(failures.to_string(), "only");
    }

    #[test]
    fn failures_display_multiple() {
        let mut failures = Failures::new("first".to_string());
        failures.push("second".to_string());
        assert_eq!(failures.to_string(), "2 errors:\n  1. first\n  2. second");
    }

    #[test]
    fn failures_is_usable_as_boxed_error() {
        #[derive(Debug)]
        struct MyError(&'static str);
        impl Display for MyError {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
        impl std::error::Error for MyError {}

        let failures = Failures::new(MyError("boom"));
        let boxed: Box<dyn std::error::Error> = Box::new(failures);
        assert_eq!(boxed.to_string(), "boom");
        assert!(boxed.source().is_some());
    }

    #[test]
    fn maybe_failures_empty_into_option_is_none() {
        let errors: MaybeFailures<String> = MaybeFailures::new();
        assert!(errors.is_empty());
        assert!(errors.into_option().is_none());
    }

    #[test]
    fn maybe_failures_push_then_into_option_is_some() {
        let mut errors = MaybeFailures::new();
        errors.push("a".to_string());
        errors.push("b".to_string());
        assert!(!errors.is_empty());

        let failures = errors.into_option().expect("two errors accumulated");
        assert_eq!(failures.len().get(), 2);
    }

    #[test]
    fn maybe_failures_default_is_empty() {
        let errors: MaybeFailures<String> = MaybeFailures::default();
        assert!(errors.is_empty());
    }

    #[test]
    fn maybe_failures_extend_accumulates() {
        let mut errors = MaybeFailures::new();
        errors.extend(vec!["a".to_string(), "b".to_string(), "c".to_string()]);
        assert_eq!(errors.into_option().expect("three errors").len().get(), 3);
    }

    #[test]
    fn ok_maybe_to_result_ok_arm() {
        let value: OkMaybe<i32, String> = OkMaybe(7, None);
        assert_eq!(value.to_result(), Ok(7));
    }

    #[test]
    fn ok_maybe_to_result_err_arm() {
        let value: OkMaybe<i32, String> = OkMaybe(7, Some("bad".to_string()));
        assert_eq!(value.to_result(), Err("bad".to_string()));
    }

    #[test]
    fn ok_maybe_push_unwrap_drains_into_maybe_failures() {
        let mut errors: MaybeFailures<String> = MaybeFailures::new();

        let first = OkMaybe::<i32, Failures<String>>(1, None).push_unwrap(&mut errors);
        let second =
            OkMaybe(2, Some(Failures::new("boom".to_string()))).push_unwrap(&mut errors);

        assert_eq!(first, 1);
        assert_eq!(second, 2);

        let failures = errors.into_option().expect("one error accumulated");
        assert_eq!(failures.into_iter().collect::<Vec<_>>(), vec!["boom".to_string()]);
    }

    #[test]
    fn ok_maybe_push_unwrap_accepts_vec_target() {
        let mut errors: Vec<String> = Vec::new();
        let value =
            OkMaybe("data", Some(Failures::new("oops".to_string()))).push_unwrap(&mut errors);
        assert_eq!(value, "data");
        assert_eq!(errors, vec!["oops".to_string()]);
    }
}
