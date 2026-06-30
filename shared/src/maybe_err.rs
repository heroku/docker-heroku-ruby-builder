//! Error-accumulation building blocks.
//!
//! Typically Rust's Result based errors fail fast, but sometimes you want to accumulate
//! as many errors as possible and present them all at once. That's the core philosophy explored in
//! the blog post ["A daft proc-macro trick"](https://schneems.com/2025/03/26/a-daft-procmacro-trick-how-to-emit-partialcode-errors/).
//!
//! An example would be parsing multiple versions from a file. If one version is unparseable, the program
//! might still want to continue execution on the ones that were valid rather than returning early.
//! The structures in this module make such deferred error decision making easier.

/// Extension methods for any iterator of `Result<T, E>` that help accumulate errors instead
/// of failing on the first one.
///
/// This covers the same ground as itertools' `partition_result`, but without pulling in the
/// itertools dependency and without the type annotations its iterator-based API requires.
/// Bring this trait into scope and the methods become available on anything that iterates over
/// `Result<T, E>` (any [`IntoIterator`]) — [`Vec`], arrays, iterator adaptors, and so on.
///
/// ```
/// use shared::maybe_err::ResultIterExt;
///
/// let results = vec![Ok(1), Err("bad"), Ok(2), Err("worse")];
/// let (oks, errs) = results.partition_result_vec();
/// assert_eq!(oks, vec![1, 2]);
/// assert_eq!(errs, vec!["bad", "worse"]);
/// ```
pub trait ResultIterExt<T, E>: IntoIterator<Item = Result<T, E>> + Sized {
    /// Return the `Ok` values, draining the errors into the provided sink.
    ///
    /// Errors are converted via [`Into`] as they are moved into `errors`, so the sink can
    /// collect a different (e.g. boxed or stringified) error type. Use this when you want to
    /// keep going with the successful values while accumulating failures somewhere else.
    ///
    /// ```
    /// use shared::maybe_err::ResultIterExt;
    ///
    /// let results = vec![Ok(1), Err("bad"), Ok(2), Err("worse")];
    ///
    /// let mut errors: Vec<String> = Vec::new();
    /// let oks = results.unwrap_drain(&mut errors);
    ///
    /// assert_eq!(oks, vec![1, 2]);
    /// assert_eq!(errors, vec!["bad".to_string(), "worse".to_string()]);
    /// ```
    #[must_use]
    fn unwrap_drain<X, Item>(self, errors: &mut X) -> Vec<T>
    where
        X: Extend<Item>,
        E: Into<Item>,
    {
        let (oks, errs) = self.partition_result_vec();
        errors.extend(errs.into_iter().map(Into::into));

        oks
    }

    /// Split the results into successes and errors, preserving order within each group.
    ///
    /// This mirrors itertools' `partition_result`: every `Ok` value lands in the first
    /// `Vec` and every `Err` value in the second, so you can accumulate all failures
    /// instead of bailing on the first one.
    ///
    /// This is a convenience wrapper over [`partition_result_iter`](Self::partition_result_iter) that fixes
    /// both output collections to `Vec`, so (unlike itertools) you don't need any type
    /// annotations and don't pull in an extra dependency.
    ///
    /// ```
    /// use shared::maybe_err::ResultIterExt;
    ///
    /// let results = vec![Ok(1), Err("bad"), Ok(2), Err("worse")];
    /// let (oks, errs) = results.partition_result_vec();
    /// assert_eq!(oks, vec![1, 2]);
    /// assert_eq!(errs, vec!["bad", "worse"]);
    /// ```
    #[must_use]
    fn partition_result_vec(self) -> (Vec<T>, Vec<E>) {
        self.partition_result_iter()
    }

    /// Split the results into successes and errors, collecting each side into a caller-chosen
    /// collection.
    ///
    /// This behaves exactly like itertools' `partition_result`: it is generic over the output
    /// collections, so you pick what each side collects into (and, like itertools, you'll
    /// usually need a type annotation to drive that choice). For the common case where both
    /// sides are `Vec`, reach for [`partition_result_vec`](Self::partition_result_vec) instead
    /// which doesn't need type annotations.
    ///
    /// ```
    /// use shared::maybe_err::ResultIterExt;
    /// use std::collections::VecDeque;
    ///
    /// let results = vec![Ok(1), Err("bad"), Ok(2), Err("worse")];
    /// let (oks, errs): (Vec<_>, VecDeque<_>) = results.partition_result_iter();
    /// assert_eq!(oks, vec![1, 2]);
    /// assert_eq!(errs, VecDeque::from(vec!["bad", "worse"]));
    /// ```
    #[must_use]
    fn partition_result_iter<A, B>(self) -> (A, B)
    where
        A: Default + Extend<T>,
        B: Default + Extend<E>,
    {
        let mut oks = A::default();
        let mut errs = B::default();
        for result in self {
            match result {
                Ok(value) => oks.extend(std::iter::once(value)),
                Err(err) => errs.extend(std::iter::once(err)),
            }
        }
        (oks, errs)
    }
}

impl<T, E, I> ResultIterExt<T, E> for I where I: IntoIterator<Item = Result<T, E>> {}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn partition_result_vec_splits_oks_and_errs_in_order() {
        let results = vec![Ok(1), Err("bad"), Ok(2), Err("worse"), Ok(3)];
        let (oks, errs) = results.partition_result_vec();
        assert_eq!(oks, vec![1, 2, 3]);
        assert_eq!(errs, vec!["bad", "worse"]);
    }

    #[test]
    fn unwrap_drain_returns_oks_and_fills_error_sink() {
        let results = vec![Ok(1), Err("bad"), Ok(2), Err("worse"), Ok(3)];

        let mut errors: Vec<String> = Vec::new();
        let oks = results.unwrap_drain(&mut errors);

        assert_eq!(oks, vec![1, 2, 3]);
        assert_eq!(errors, vec!["bad".to_string(), "worse".to_string()]);
    }

    #[test]
    fn partition_result_vec_handles_empty() {
        let results: Vec<Result<i32, &str>> = vec![];
        let (oks, errs) = results.partition_result_vec();
        assert!(oks.is_empty());
        assert!(errs.is_empty());
    }

    #[test]
    fn partition_result_vec_works_on_any_into_iterator() {
        let results: [Result<i32, &str>; 3] = [Ok(1), Err("bad"), Ok(2)];
        let (oks, errs) = results.partition_result_vec();
        assert_eq!(oks, vec![1, 2]);
        assert_eq!(errs, vec!["bad"]);

        let (oks, errs) = (1..=4)
            .map(|n| if n % 2 == 0 { Ok(n) } else { Err(n) })
            .partition_result_vec();
        assert_eq!(oks, vec![2, 4]);
        assert_eq!(errs, vec![1, 3]);
    }
}
