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
//! - [`MaybeErrors`] -- zero or more errors (the empty-able accumulator you push into).
//! - [`MultiErrors`] -- one or more errors (the non-empty value you return).
//! - [`OkMaybe`] -- a value plus maybe one-or-more errors.
//!
//! The error behavior of a function is encoded in its return type:
//!
//! - Errors that block producing a value: `Result<T, MultiErrors<E>>`.
//! - Errors that never block: `OkMaybe<T, MultiErrors<E>>`.
//! - Errors that may or may not block: `Result<OkMaybe<T, MultiErrors<E>>, MultiErrors<E>>`.
//!   - `Ok(OkMaybe(value, None))` -- no errors.
//!   - `Ok(OkMaybe(value, Some(multi_errors)))` -- error(s) that did not block.
//!   - `Err(multi_errors)` -- could not produce a value due to error(s).

use std::fmt::{self, Display};
use std::num::NonZeroUsize;

/// One or more errors, guaranteed non-empty by construction.
///
/// This is the value you *return* when you have at least one error. It fills the
/// role `syn::Error` plays in `proc_micro`: a single value that always holds at
/// least one error and can hold many.
///
/// Build the first error with [`MultiErrors::new`], then add more with
/// [`MultiErrors::push`] or [`Extend`]. To collapse a possibly-empty pile of errors
/// into `Option<MultiErrors<E>>`, accumulate into a [`MaybeErrors`] instead.
///
/// ```
/// use shared::maybe_err::MultiErrors;
///
/// let mut multi_errors = MultiErrors::new("first".to_string());
/// multi_errors.push("second".to_string());
///
/// assert_eq!(multi_errors.len().get(), 2);
/// let collected: Vec<String> = multi_errors.into_iter().collect();
/// assert_eq!(collected, vec!["first".to_string(), "second".to_string()]);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultiErrors<E> {
    head: E,
    tail: Vec<E>,
}

impl<E> MultiErrors<E> {
    /// Create a non-empty collection from a single error.
    ///
    /// ```
    /// use shared::maybe_err::MultiErrors;
    ///
    /// let multi_errors = MultiErrors::new("boom".to_string());
    /// assert_eq!(multi_errors.len().get(), 1);
    /// ```
    pub fn new(first: E) -> Self {
        MultiErrors {
            head: first,
            tail: Vec::new(),
        }
    }

    /// Append another error.
    ///
    /// ```
    /// use shared::maybe_err::MultiErrors;
    ///
    /// let mut multi_errors = MultiErrors::new("a".to_string());
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
    /// use shared::maybe_err::MultiErrors;
    ///
    /// let multi_errors = MultiErrors::new(());
    /// assert_eq!(multi_errors.len().get(), 1);
    /// ```
    pub fn len(&self) -> NonZeroUsize {
        // SAFETY-equivalent: `1 + tail.len()` is always >= 1, so the unwrap cannot fail.
        NonZeroUsize::new(1 + self.tail.len()).expect("MultiErrors always holds at least one error")
    }

    /// Borrow each error in turn, head first then the rest in push order.
    ///
    /// ```
    /// use shared::maybe_err::MultiErrors;
    ///
    /// let mut multi_errors = MultiErrors::new(1);
    /// multi_errors.push(2);
    /// multi_errors.push(3);
    /// assert_eq!(multi_errors.iter().copied().collect::<Vec<_>>(), vec![1, 2, 3]);
    /// ```
    pub fn iter(&self) -> std::iter::Chain<std::iter::Once<&E>, std::slice::Iter<'_, E>> {
        std::iter::once(&self.head).chain(self.tail.iter())
    }
}

impl<E> IntoIterator for MultiErrors<E> {
    type Item = E;
    type IntoIter = std::iter::Chain<std::iter::Once<E>, std::vec::IntoIter<E>>;

    /// Iterate over every error, head first then the rest in push order.
    ///
    /// ```
    /// use shared::maybe_err::MultiErrors;
    ///
    /// let mut multi_errors = MultiErrors::new(1);
    /// multi_errors.push(2);
    /// multi_errors.push(3);
    /// assert_eq!(multi_errors.into_iter().collect::<Vec<_>>(), vec![1, 2, 3]);
    /// ```
    fn into_iter(self) -> Self::IntoIter {
        std::iter::once(self.head).chain(self.tail)
    }
}

impl<'a, E> IntoIterator for &'a MultiErrors<E> {
    type Item = &'a E;
    type IntoIter = std::iter::Chain<std::iter::Once<&'a E>, std::slice::Iter<'a, E>>;

    /// Borrowing iteration, so `for err in &multi_errors` works.
    ///
    /// ```
    /// use shared::maybe_err::MultiErrors;
    ///
    /// let mut multi_errors = MultiErrors::new(1);
    /// multi_errors.push(2);
    /// let seen: Vec<i32> = (&multi_errors).into_iter().copied().collect();
    /// assert_eq!(seen, vec![1, 2]);
    /// ```
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<E> Extend<E> for MultiErrors<E> {
    /// Append many errors at once. Also serves as a "combine": extend one
    /// `MultiErrors` with the contents of another via its [`IntoIterator`].
    ///
    /// ```
    /// use shared::maybe_err::MultiErrors;
    ///
    /// let mut a = MultiErrors::new(1);
    /// let mut b = MultiErrors::new(2);
    /// b.push(3);
    /// a.extend(b);
    /// assert_eq!(a.into_iter().collect::<Vec<_>>(), vec![1, 2, 3]);
    /// ```
    fn extend<I: IntoIterator<Item = E>>(&mut self, iter: I) {
        self.tail.extend(iter);
    }
}

impl<E: Display> Display for MultiErrors<E> {
    /// A single error renders as that error; multiple render as a numbered block.
    ///
    /// ```
    /// use shared::maybe_err::MultiErrors;
    ///
    /// let one = MultiErrors::new("only".to_string());
    /// assert_eq!(one.to_string(), "only");
    ///
    /// let mut many = MultiErrors::new("first".to_string());
    /// many.push("second".to_string());
    /// assert_eq!(many.to_string(), "2 errors:\n  1. first\n  2. second");
    /// ```
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let len = self.len();
        if len.get() == 1 {
            write!(f, "{}", self.head)
        } else {
            write!(f, "{} errors:", len)?;
            for (index, err) in std::iter::once(&self.head)
                .chain(self.tail.iter())
                .enumerate()
            {
                write!(f, "\n  {}. {}", index + 1, err)?;
            }
            Ok(())
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for MultiErrors<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.head)
    }
}

/// Zero or more errors: the empty-able accumulator you push into.
///
/// This fills the role of `proc_micro`'s `MaybeError`. It starts empty and, once
/// you are done accumulating, [`MaybeErrors::into_option`] collapses it into
/// `Option<MultiErrors<E>>`: `None` means there were no errors, `Some` means one or
/// more.
///
/// ```
/// use shared::maybe_err::{MultiErrors, MaybeErrors};
///
/// let mut errors = MaybeErrors::new();
/// assert!(errors.is_empty());
///
/// errors.push("nope".to_string());
/// errors.push("also nope".to_string());
///
/// let multi_errors: MultiErrors<String> = errors.into_option().expect("two errors");
/// assert_eq!(multi_errors.len().get(), 2);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaybeErrors<E>(Option<MultiErrors<E>>);

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
    /// [`MultiErrors`]; later pushes append to it.
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
            none => *none = Some(MultiErrors::new(err)),
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

    /// Collapse into `Option<MultiErrors<E>>`: `None` if empty, otherwise the
    /// accumulated non-empty [`MultiErrors`].
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
    pub fn into_option(self) -> Option<MultiErrors<E>> {
        self.0
    }

    pub fn ok_maybe<T>(self, t: T) -> OkMaybe<T, MultiErrors<E>> {
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

/// A value and maybe an error.
///
/// `OkMaybe(value, None)` carries a value with no error; `OkMaybe(value,
/// Some(err))` carries a value alongside an error. Because it is not a `Result`,
/// the `?` operator cannot accidentally discard the value via an early return,
/// which encourages accumulating errors rather than bailing on the first one.
///
/// The error type is generic. Pair it with [`MultiErrors`] (`OkMaybe<T,
/// MultiErrors<E>>`) when the error half should itself be able to hold one or more
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
    /// The error type must be [`IntoIterator`] (as [`MultiErrors`] is), so its
    /// errors can be pushed into any [`Extend`] target such as a
    /// [`MaybeErrors`] or a plain `Vec`. This is the key ergonomic for
    /// accumulating across many fallible steps in a loop.
    ///
    /// ```
    /// use shared::maybe_err::{MultiErrors, MaybeErrors, OkMaybe};
    ///
    /// let mut errors: MaybeErrors<String> = MaybeErrors::new();
    ///
    /// let value = OkMaybe(10, Some(MultiErrors::new("bad".to_string())))
    ///     .drain_unwrap(&mut errors);
    ///
    /// assert_eq!(value, 10);
    /// assert!(!errors.is_empty());
    /// ```
    pub fn drain_unwrap<G>(self, push_to: &mut impl Extend<G>) -> T
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
    fn multi_errors_len_starts_at_one() {
        let multi_errors = MultiErrors::new("a");
        assert_eq!(multi_errors.len().get(), 1);
    }

    #[test]
    fn multi_errors_push_grows_len() {
        let mut multi_errors = MultiErrors::new("a");
        multi_errors.push("b");
        multi_errors.push("c");
        assert_eq!(multi_errors.len().get(), 3);
    }

    #[test]
    fn multi_errors_into_iter_is_head_then_tail() {
        let mut multi_errors = MultiErrors::new(1);
        multi_errors.push(2);
        multi_errors.push(3);
        assert_eq!(multi_errors.into_iter().collect::<Vec<_>>(), vec![1, 2, 3]);
    }

    #[test]
    fn multi_errors_extend_appends() {
        let mut multi_errors = MultiErrors::new(1);
        multi_errors.extend(vec![2, 3]);

        let mut other = MultiErrors::new(4);
        other.push(5);
        multi_errors.extend(other);

        assert_eq!(
            multi_errors.into_iter().collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5]
        );
    }

    #[test]
    fn multi_errors_display_single() {
        let multi_errors = MultiErrors::new("only".to_string());
        assert_eq!(multi_errors.to_string(), "only");
    }

    #[test]
    fn multi_errors_display_multiple() {
        let mut multi_errors = MultiErrors::new("first".to_string());
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

        let multi_errors = MultiErrors::new(MyError("boom"));
        let boxed: Box<dyn std::error::Error> = Box::new(multi_errors);
        assert_eq!(boxed.to_string(), "boom");
        assert!(boxed.source().is_some());
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

        let first = OkMaybe::<i32, MultiErrors<String>>(1, None).drain_unwrap(&mut errors);
        let second =
            OkMaybe(2, Some(MultiErrors::new("boom".to_string()))).drain_unwrap(&mut errors);

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
        let value =
            OkMaybe("data", Some(MultiErrors::new("oops".to_string()))).drain_unwrap(&mut errors);
        assert_eq!(value, "data");
        assert_eq!(errors, vec!["oops".to_string()]);
    }
}
