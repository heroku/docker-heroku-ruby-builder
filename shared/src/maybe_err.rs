//! Error-accumulation building blocks.
//!
//! Typically Rust's Result based errors fail fast, but sometimes you want to accumulate
//! as many errors as possible and present them all at once. That's the core philosophy explored in
//! the blog post ["A daft proc-macro trick"](https://schneems.com/2025/03/26/a-daft-procmacro-trick-how-to-emit-partialcode-errors/).
//!
//! An example would be parsing multiple versions from a file. If one version is unparseable, the program
//! might still want to continue execution on the ones that were valid rather than returning early.
//! The structures in this module make such deferred error decision making easier.
//!
//! ## Structs
//!
//! - [`MaybeErrors`] - Zero or more errors (the empty-able accumulator you push into).
//! - [`NonEmptyErrors`] - One or more errors (the non-empty value you return).
//! - [`OkMaybe`] -- A value plus maybe one-or-more errors.
//!
//! ## Example
//!
//! In a function that returns an [`OkMaybe`], it's common to build an error accumulator
//! early and delay evaluation of the return result until the end. Push into the
//! accumulator as you go (or use [`OkMaybe::drain_unwrap`] to drain a sub-result's
//! errors into it while keeping the value), then hand it the value with
//! [`MaybeErrors::ok_maybe`]:
//!
//! ```
//! use shared::maybe_err::{MaybeErrors, NonEmptyErrors, OkMaybe};
//!
//! /// Parse every input, keeping the ones that succeed and collecting the rest as errors.
//! fn parse_all(inputs: &[&str]) -> OkMaybe<Vec<u32>, NonEmptyErrors<String>> {
//!     let mut errors = MaybeErrors::new();
//!     let mut values = Vec::new();
//!
//!     for input in inputs {
//!         match input.parse::<u32>() {
//!             Ok(value) => values.push(value),
//!             Err(err) => errors.push(format!("{input:?}: {err}")),
//!         }
//!     }
//!
//!     errors.ok_maybe(values)
//! }
//!
//! // All inputs valid: a value and no errors.
//! assert_eq!(parse_all(&["1", "2", "3"]), OkMaybe(vec![1, 2, 3], None));
//!
//! // Some inputs invalid: the good values plus the accumulated errors.
//! let OkMaybe(values, maybe) = parse_all(&["1", "nope", "3", "also nope"]);
//! assert_eq!(values, vec![1, 3]);
//! assert_eq!(maybe.expect("two errors").len().get(), 2);
//! ```
//!
//! ## Guidance
//!
//! The error behavior of a function is encoded in its return type:
//!
//! - Errors that block producing a value: `Result<T, NonEmptyErrors<E>>`. This represents either a valid
//!   type `T` or 1 or more errors.
//! - Errors that never block producing a value — the caller decides whether to surface them:
//!   `OkMaybe<T, NonEmptyErrors<E>>`. In this representation `T` is always
//!   available, but there may or may not also be errors. If there are errors, `NonEmptyErrors` represents
//!   1 or more error.
//! - Errors that may or may not block: `Result<OkMaybe<T, NonEmptyErrors<E>>, NonEmptyErrors<E>>`.
//!   - `Ok(OkMaybe(value, None))` -- no errors.
//!   - `Ok(OkMaybe(value, Some(multi_errors)))` -- error(s) that did not block.
//!   - `Err(multi_errors)` -- could not produce a value due to error(s).

use std::fmt::{self, Display};
use std::num::NonZeroUsize;

/// One or more errors, guaranteed non-empty by construction.
///
/// This is the value you *return* when you have at least one error. If you need to represent
/// zero or more errors you can use [`MaybeErrors`] instead.
///
/// Build the first error with [`NonEmptyErrors::new`], then add more with
/// [`NonEmptyErrors::push`] or [`Extend`]. To collapse a possibly-empty pile of errors
/// into `Option<NonEmptyErrors<E>>`, accumulate into a [`MaybeErrors`] instead.
///
/// ```
/// use shared::maybe_err::NonEmptyErrors;
///
/// let mut multi_errors = NonEmptyErrors::new("first".to_string());
/// multi_errors.push("second".to_string());
///
/// assert_eq!(multi_errors.len().get(), 2);
/// let collected: Vec<String> = multi_errors.into_iter().collect();
/// assert_eq!(collected, vec!["first".to_string(), "second".to_string()]);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NonEmptyErrors<E> {
    head: E,
    tail: Vec<E>,
}

impl<E> NonEmptyErrors<E> {
    /// Create a non-empty collection from a single error.
    ///
    /// ```
    /// use shared::maybe_err::NonEmptyErrors;
    ///
    /// let multi_errors = NonEmptyErrors::new("boom".to_string());
    /// assert_eq!(multi_errors.len().get(), 1);
    /// ```
    pub fn new(first: E) -> Self {
        NonEmptyErrors {
            head: first,
            tail: Vec::new(),
        }
    }

    /// Append another error.
    ///
    /// ```
    /// use shared::maybe_err::NonEmptyErrors;
    ///
    /// let mut multi_errors = NonEmptyErrors::new("a".to_string());
    /// multi_errors.push("b".to_string());
    /// assert_eq!(multi_errors.len().get(), 2);
    /// ```
    pub fn push(&mut self, err: E) {
        self.tail.push(err);
    }

    /// The number of errors held, always at least one.
    ///
    /// Returning [`NonZeroUsize`] surfaces the non-empty guarantee in the type.
    ///
    /// ```
    /// use shared::maybe_err::NonEmptyErrors;
    ///
    /// let multi_errors = NonEmptyErrors::new(());
    /// assert_eq!(multi_errors.len().get(), 1);
    /// ```
    pub fn len(&self) -> NonZeroUsize {
        NonZeroUsize::new(1 + self.tail.len())
            .expect("NonEmptyErrors always holds at least one error")
    }

    /// Borrow each error in turn, head first then the rest in push order.
    ///
    /// ```
    /// use shared::maybe_err::NonEmptyErrors;
    ///
    /// let mut multi_errors = NonEmptyErrors::new(1);
    /// multi_errors.push(2);
    /// multi_errors.push(3);
    /// assert_eq!(multi_errors.iter().copied().collect::<Vec<_>>(), vec![1, 2, 3]);
    /// ```
    pub fn iter(&self) -> std::iter::Chain<std::iter::Once<&E>, std::slice::Iter<'_, E>> {
        std::iter::once(&self.head).chain(self.tail.iter())
    }
}

impl<E> IntoIterator for NonEmptyErrors<E> {
    type Item = E;
    type IntoIter = std::iter::Chain<std::iter::Once<E>, std::vec::IntoIter<E>>;

    /// Iterate over every error, head first then the rest in push order.
    ///
    /// ```
    /// use shared::maybe_err::NonEmptyErrors;
    ///
    /// let mut multi_errors = NonEmptyErrors::new(1);
    /// multi_errors.push(2);
    /// multi_errors.push(3);
    /// assert_eq!(multi_errors.into_iter().collect::<Vec<_>>(), vec![1, 2, 3]);
    /// ```
    fn into_iter(self) -> Self::IntoIter {
        std::iter::once(self.head).chain(self.tail)
    }
}

impl<'a, E> IntoIterator for &'a NonEmptyErrors<E> {
    type Item = &'a E;
    type IntoIter = std::iter::Chain<std::iter::Once<&'a E>, std::slice::Iter<'a, E>>;

    /// Borrowing iteration, so `for err in &multi_errors` works.
    ///
    /// ```
    /// use shared::maybe_err::NonEmptyErrors;
    ///
    /// let mut multi_errors = NonEmptyErrors::new(1);
    /// multi_errors.push(2);
    /// let seen: Vec<i32> = (&multi_errors).into_iter().copied().collect();
    /// assert_eq!(seen, vec![1, 2]);
    /// ```
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<E> Extend<E> for NonEmptyErrors<E> {
    /// Append many errors at once. Also serves as a "combine": extend one
    /// `NonEmptyErrors` with the contents of another via its [`IntoIterator`].
    ///
    /// ```
    /// use shared::maybe_err::NonEmptyErrors;
    ///
    /// let mut a = NonEmptyErrors::new(1);
    /// let mut b = NonEmptyErrors::new(2);
    /// b.push(3);
    /// a.extend(b);
    /// assert_eq!(a.into_iter().collect::<Vec<_>>(), vec![1, 2, 3]);
    /// ```
    fn extend<I: IntoIterator<Item = E>>(&mut self, iter: I) {
        self.tail.extend(iter);
    }
}

impl<E> From<E> for NonEmptyErrors<E> {
    /// Lift a single error into a `NonEmptyErrors`, enabling `.into()` and `?`.
    ///
    /// ```
    /// use shared::maybe_err::NonEmptyErrors;
    ///
    /// let errs: NonEmptyErrors<String> = "boom".to_string().into();
    /// assert_eq!(errs.len().get(), 1);
    /// ```
    fn from(err: E) -> Self {
        NonEmptyErrors::new(err)
    }
}

impl<E: Display> Display for NonEmptyErrors<E> {
    /// A single error renders as that error; multiple render as a numbered block.
    ///
    /// ```
    /// use shared::maybe_err::NonEmptyErrors;
    ///
    /// let one = NonEmptyErrors::new("only".to_string());
    /// assert_eq!(one.to_string(), "only");
    ///
    /// let mut many = NonEmptyErrors::new("first".to_string());
    /// many.push("second".to_string());
    /// assert_eq!(many.to_string(), "2 errors:\n  1. first\n  2. second");
    /// ```
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let len = self.len();
        if len.get() == 1 {
            write!(f, "{}", self.head)
        } else {
            write!(f, "{} errors:", len)?;
            for (index, err) in self.iter().enumerate() {
                write!(f, "\n  {}. {}", index + 1, err)?;
            }
            Ok(())
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for NonEmptyErrors<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }
}

/// Zero or more errors: the empty-able accumulator you push into.
///
/// It starts empty and, once you are done accumulating, [`MaybeErrors::into_option`] collapses it into
/// `Option<NonEmptyErrors<E>>`: `None` means there were no errors, `Some` means one or
/// more.
///
/// ```
/// use shared::maybe_err::{NonEmptyErrors, MaybeErrors};
///
/// let mut errors = MaybeErrors::new();
/// assert!(errors.is_empty());
///
/// errors.push("nope".to_string());
/// errors.push("also nope".to_string());
///
/// let multi_errors: NonEmptyErrors<String> = errors.into_option().expect("two errors");
/// assert_eq!(multi_errors.len().get(), 2);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaybeErrors<E>(Option<NonEmptyErrors<E>>);

impl<E> MaybeErrors<E> {
    /// Create an empty accumulator.
    ///
    /// ```
    /// use shared::maybe_err::MaybeErrors;
    ///
    /// let errors: MaybeErrors<String> = MaybeErrors::new();
    /// assert!(errors.is_empty());
    /// ```
    pub fn new() -> Self {
        MaybeErrors(None)
    }

    /// Accumulate one error. The first push creates the underlying
    /// [`NonEmptyErrors`]; later pushes append to it.
    ///
    /// ```
    /// use shared::maybe_err::MaybeErrors;
    ///
    /// let mut errors = MaybeErrors::new();
    /// errors.push("boom".to_string());
    /// assert!(!errors.is_empty());
    /// ```
    pub fn push(&mut self, err: E) {
        match &mut self.0 {
            Some(multi_errors) => multi_errors.push(err),
            none => *none = Some(NonEmptyErrors::new(err)),
        }
    }

    /// Whether any error has been accumulated yet.
    ///
    /// ```
    /// use shared::maybe_err::MaybeErrors;
    ///
    /// let mut errors = MaybeErrors::new();
    /// assert!(errors.is_empty());
    /// errors.push(());
    /// assert!(!errors.is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.0.is_none()
    }

    /// How many errors have been accumulated, `0` when empty.
    ///
    /// ```
    /// use shared::maybe_err::MaybeErrors;
    ///
    /// let mut errors = MaybeErrors::new();
    /// assert_eq!(errors.len(), 0);
    /// errors.push("a".to_string());
    /// errors.push("b".to_string());
    /// assert_eq!(errors.len(), 2);
    /// ```
    pub fn len(&self) -> usize {
        self.0
            .as_ref()
            .map_or(0, |multi_errors| multi_errors.len().get())
    }

    /// Borrow each accumulated error in turn; yields nothing when empty.
    ///
    /// ```
    /// use shared::maybe_err::MaybeErrors;
    ///
    /// let mut errors = MaybeErrors::new();
    /// errors.push("a".to_string());
    /// errors.push("b".to_string());
    /// let seen: Vec<&String> = errors.iter().collect();
    /// assert_eq!(seen, vec![&"a".to_string(), &"b".to_string()]);
    /// ```
    pub fn iter(&self) -> impl Iterator<Item = &E> {
        self.into_iter()
    }

    /// Collapse into `Option<NonEmptyErrors<E>>`: `None` if empty, otherwise the
    /// accumulated non-empty [`NonEmptyErrors`].
    ///
    /// ```
    /// use shared::maybe_err::MaybeErrors;
    ///
    /// let empty: MaybeErrors<String> = MaybeErrors::new();
    /// assert!(empty.into_option().is_none());
    ///
    /// let mut errors = MaybeErrors::new();
    /// errors.push("boom".to_string());
    /// assert!(errors.into_option().is_some());
    /// ```
    pub fn into_option(self) -> Option<NonEmptyErrors<E>> {
        self.0
    }

    /// Pair the accumulated errors with a value, producing an [`OkMaybe`].
    ///
    /// This is the bridge from the accumulate phase to the return phase: once
    /// you have finished pushing into a `MaybeErrors` and have produced a value,
    /// `ok_maybe` collapses the accumulator into the error half of an
    /// `OkMaybe<T, NonEmptyErrors<E>>`. An empty accumulator yields
    /// `OkMaybe(value, None)`; a non-empty one yields `OkMaybe(value,
    /// Some(multi_errors))`.
    ///
    /// ```
    /// use shared::maybe_err::{MaybeErrors, OkMaybe};
    ///
    /// let errors: MaybeErrors<String> = MaybeErrors::new();
    /// assert_eq!(errors.ok_maybe(1), OkMaybe(1, None));
    ///
    /// let mut errors = MaybeErrors::new();
    /// errors.push("boom".to_string());
    /// let OkMaybe(value, maybe) = errors.ok_maybe(2);
    /// assert_eq!(value, 2);
    /// assert_eq!(maybe.expect("one error").len().get(), 1);
    /// ```
    pub fn ok_maybe<T>(self, t: T) -> OkMaybe<T, NonEmptyErrors<E>> {
        match self.0 {
            Some(inner) => OkMaybe(t, Some(inner)),
            None => OkMaybe(t, None),
        }
    }
}

impl<E> Default for MaybeErrors<E> {
    fn default() -> Self {
        MaybeErrors::new()
    }
}

impl<'a, E> IntoIterator for &'a MaybeErrors<E> {
    type Item = &'a E;
    type IntoIter = std::iter::Flatten<
        std::option::IntoIter<std::iter::Chain<std::iter::Once<&'a E>, std::slice::Iter<'a, E>>>,
    >;

    /// Borrowing iteration, so `for err in &errors` works. An empty accumulator
    /// yields no items.
    ///
    /// ```
    /// use shared::maybe_err::MaybeErrors;
    ///
    /// let empty: MaybeErrors<String> = MaybeErrors::new();
    /// assert_eq!((&empty).into_iter().count(), 0);
    ///
    /// let mut errors = MaybeErrors::new();
    /// errors.push(1);
    /// errors.push(2);
    /// let seen: Vec<i32> = (&errors).into_iter().copied().collect();
    /// assert_eq!(seen, vec![1, 2]);
    /// ```
    fn into_iter(self) -> Self::IntoIter {
        self.0
            .as_ref()
            .map(IntoIterator::into_iter)
            .into_iter()
            .flatten()
    }
}

impl<E> Extend<E> for MaybeErrors<E> {
    /// Accumulate many errors at once. This is what makes a `MaybeErrors` a
    /// valid [`OkMaybe::drain_unwrap`] target.
    ///
    /// ```
    /// use shared::maybe_err::MaybeErrors;
    ///
    /// let mut errors = MaybeErrors::new();
    /// errors.extend(vec!["a".to_string(), "b".to_string()]);
    /// assert_eq!(errors.into_option().expect("two errors").len().get(), 2);
    /// ```
    fn extend<I: IntoIterator<Item = E>>(&mut self, iter: I) {
        for err in iter {
            self.push(err);
        }
    }
}

impl<E> FromIterator<E> for MaybeErrors<E> {
    /// Collect errors into an accumulator, so `iter.collect::<MaybeErrors<_>>()` works.
    ///
    /// ```
    /// use shared::maybe_err::MaybeErrors;
    ///
    /// let errors: MaybeErrors<String> =
    ///     vec!["a".to_string(), "b".to_string()].into_iter().collect();
    /// assert_eq!(errors.len(), 2);
    ///
    /// let empty: MaybeErrors<String> = std::iter::empty().collect();
    /// assert!(empty.is_empty());
    /// ```
    fn from_iter<I: IntoIterator<Item = E>>(iter: I) -> Self {
        let mut errors = MaybeErrors::new();
        errors.extend(iter);
        errors
    }
}

/// A value paired with maybe an error.
///
/// A replacement for `Result<T, E>` when we always want to produce `T` and
/// sometimes emit error(s) alongside it.
///
/// A function returning `Result<T, E>` may finish without ever constructing a
/// `T`. In a function returning `OkMaybe<T, E>`: every return path must
/// produce a `T`. This rules out `?`-style short-circuit returns and
/// reinforces error accumulation at the type-signature level.
///
/// Variants:
///
/// - `OkMaybe(value, None)` carries a value with no error
/// - `OkMaybe(value, Some(err))` carries a value AND an error.
///
/// To return multiple errors we suggest using `OkMaybe<T, NonEmptyErrors<E>>`.
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
///
/// In a function that returns an [`OkMaybe`], it's common to build an error accumulator
/// early and delay evaluation of the return result until the end. Push into the
/// accumulator as you go (or [`OkMaybe::drain_unwrap`] sub-results into it), then
/// hand it the value with [`MaybeErrors::ok_maybe`]:
///
/// ```
/// use shared::maybe_err::{MaybeErrors, NonEmptyErrors, OkMaybe};
///
/// /// Parse every input, keeping the ones that succeed and collecting the rest as errors.
/// fn parse_all(inputs: &[&str]) -> OkMaybe<Vec<u32>, NonEmptyErrors<String>> {
///     let mut errors = MaybeErrors::new();
///     let mut values = Vec::new();
///
///     for input in inputs {
///         match input.parse::<u32>() {
///             Ok(value) => values.push(value),
///             Err(err) => errors.push(format!("{input:?}: {err}")),
///         }
///     }
///
///     errors.ok_maybe(values)
/// }
///
/// // All inputs valid: a value and no errors.
/// assert_eq!(parse_all(&["1", "2", "3"]), OkMaybe(vec![1, 2, 3], None));
///
/// // Some inputs invalid: the good values plus the accumulated errors.
/// let OkMaybe(values, maybe) = parse_all(&["1", "nope", "3", "also nope"]);
/// assert_eq!(values, vec![1, 3]);
/// assert_eq!(maybe.expect("two errors").len().get(), 2);
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
    /// The error type must be [`IntoIterator`] (as [`NonEmptyErrors`] is), so its
    /// errors can be pushed into any [`Extend`] target such as a
    /// [`MaybeErrors`] or a plain `Vec`. This is the key ergonomic for
    /// accumulating across many fallible steps in a loop.
    ///
    /// The target chooses the error type it stores: each drained error is
    /// converted with [`Into`], so a sub-result with a concrete error type can
    /// be funneled into a wider accumulator such as
    /// `MaybeErrors<Box<dyn std::error::Error>>`. When the types already match,
    /// the conversion is the no-op reflexive [`From`].
    ///
    /// ```
    /// use shared::maybe_err::{NonEmptyErrors, MaybeErrors, OkMaybe};
    ///
    /// let mut errors: MaybeErrors<String> = MaybeErrors::new();
    ///
    /// // Same type drains as-is.
    /// let value = OkMaybe(10, Some(NonEmptyErrors::new("bad".to_string())))
    ///     .drain_unwrap(&mut errors);
    /// assert_eq!(value, 10);
    /// assert!(!errors.is_empty());
    ///
    /// // Source errors are `&str`, accumulator holds `String`: converted via `Into`.
    /// let value = OkMaybe(20, Some(NonEmptyErrors::new("worse")))
    ///     .drain_unwrap(&mut errors);
    /// assert_eq!(value, 20);
    /// assert_eq!(errors.len(), 2);
    /// ```
    pub fn drain_unwrap<G, H>(self, push_to: &mut impl Extend<H>) -> T
    where
        E: IntoIterator<Item = G>,
        G: Into<H>,
    {
        let OkMaybe(value, maybe) = self;
        if let Some(err) = maybe {
            push_to.extend(err.into_iter().map(Into::into));
        }
        value
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn multi_errors_len_starts_at_one() {
        let multi_errors = NonEmptyErrors::new("a");
        assert_eq!(multi_errors.len().get(), 1);
    }

    #[test]
    fn multi_errors_push_grows_len() {
        let mut multi_errors = NonEmptyErrors::new("a");
        multi_errors.push("b");
        multi_errors.push("c");
        assert_eq!(multi_errors.len().get(), 3);
    }

    #[test]
    fn multi_errors_into_iter_is_head_then_tail() {
        let mut multi_errors = NonEmptyErrors::new(1);
        multi_errors.push(2);
        multi_errors.push(3);
        assert_eq!(multi_errors.into_iter().collect::<Vec<_>>(), vec![1, 2, 3]);
    }

    #[test]
    fn multi_errors_extend_appends() {
        let mut multi_errors = NonEmptyErrors::new(1);
        multi_errors.extend(vec![2, 3]);

        let mut other = NonEmptyErrors::new(4);
        other.push(5);
        multi_errors.extend(other);

        assert_eq!(
            multi_errors.into_iter().collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5]
        );
    }

    #[test]
    fn multi_errors_display_single() {
        let multi_errors = NonEmptyErrors::new("only".to_string());
        assert_eq!(multi_errors.to_string(), "only");
    }

    #[test]
    fn multi_errors_display_multiple() {
        let mut multi_errors = NonEmptyErrors::new("first".to_string());
        multi_errors.push("second".to_string());
        assert_eq!(
            multi_errors.to_string(),
            "2 errors:\n  1. first\n  2. second"
        );
    }

    #[test]
    fn multi_errors_is_usable_as_boxed_error() {
        #[derive(Debug)]
        struct MyError(&'static str);
        impl Display for MyError {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
        impl std::error::Error for MyError {}

        let multi_errors = NonEmptyErrors::new(MyError("boom"));
        let boxed: Box<dyn std::error::Error> = Box::new(multi_errors);
        assert_eq!(boxed.to_string(), "boom");
        assert!(boxed.source().is_none());
    }

    #[test]
    fn maybe_errors_empty_into_option_is_none() {
        let errors: MaybeErrors<String> = MaybeErrors::new();
        assert!(errors.is_empty());
        assert!(errors.into_option().is_none());
    }

    #[test]
    fn maybe_errors_push_then_into_option_is_some() {
        let mut errors = MaybeErrors::new();
        errors.push("a".to_string());
        errors.push("b".to_string());
        assert!(!errors.is_empty());

        let multi_errors = errors.into_option().expect("two errors accumulated");
        assert_eq!(multi_errors.len().get(), 2);
    }

    #[test]
    fn maybe_errors_default_is_empty() {
        let errors: MaybeErrors<String> = MaybeErrors::default();
        assert!(errors.is_empty());
    }

    #[test]
    fn maybe_errors_extend_accumulates() {
        let mut errors = MaybeErrors::new();
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
    fn ok_maybe_push_unwrap_drains_into_maybe_errors() {
        let mut errors: MaybeErrors<String> = MaybeErrors::new();

        let first = OkMaybe::<i32, NonEmptyErrors<String>>(1, None).drain_unwrap(&mut errors);
        let second =
            OkMaybe(2, Some(NonEmptyErrors::new("boom".to_string()))).drain_unwrap(&mut errors);

        assert_eq!(first, 1);
        assert_eq!(second, 2);

        let multi_errors = errors.into_option().expect("one error accumulated");
        assert_eq!(
            multi_errors.into_iter().collect::<Vec<_>>(),
            vec!["boom".to_string()]
        );
    }

    #[test]
    fn ok_maybe_push_unwrap_accepts_vec_target() {
        let mut errors: Vec<String> = Vec::new();
        let value = OkMaybe("data", Some(NonEmptyErrors::new("oops".to_string())))
            .drain_unwrap(&mut errors);
        assert_eq!(value, "data");
        assert_eq!(errors, vec!["oops".to_string()]);
    }

    #[test]
    fn non_empty_errors_from_single_error() {
        let multi_errors: NonEmptyErrors<String> = "boom".to_string().into();
        assert_eq!(multi_errors.len().get(), 1);
        assert_eq!(
            multi_errors.into_iter().collect::<Vec<_>>(),
            vec!["boom".to_string()]
        );
    }

    #[test]
    fn maybe_errors_from_iter_non_empty() {
        let errors: MaybeErrors<String> =
            vec!["a".to_string(), "b".to_string()].into_iter().collect();
        assert_eq!(errors.len(), 2);
        assert_eq!(
            errors
                .into_option()
                .expect("two errors")
                .into_iter()
                .collect::<Vec<_>>(),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn maybe_errors_from_iter_empty() {
        let empty: MaybeErrors<String> = std::iter::empty().collect();
        assert!(empty.is_empty());
        assert!(empty.into_option().is_none());
    }
}
