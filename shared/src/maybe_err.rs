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
//! See [`ResultVec`] for the main accumulator type.

/// Return multiple errors and/or results
///
/// Sugared version of `Vec<Result<T, E>>` allowing accumulation of errors:
///
/// ```
/// use shared::maybe_err::ResultVec;
///
/// // Parse every entry, keeping the good values *and* the failures instead of
/// // bailing on the first unparseable one. The function return type drives the
/// // `collect`, so no turbofish is needed.
/// fn parse_versions(raw: &[&str]) -> ResultVec<u32, String> {
///     raw.iter()
///         .map(|s| s.parse::<u32>().map_err(|e| format!("{s:?}: {e}")))
///         .collect()
/// }
///
/// let parsed = parse_versions(&["1", "nope", "3"]);
///
/// // Drain the errors into a sink and keep going with the successes.
/// let mut errors: Vec<String> = Vec::new();
/// let versions = parsed.unwrap_drain_errs(&mut errors);
///
/// assert_eq!(versions, vec![1, 3]);
/// assert_eq!(errors, vec![r#""nope": invalid digit found in string"#.to_string()]);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultVec<T, E> {
    inner: Vec<Result<T, E>>,
}

impl<T, E> ResultVec<T, E> {
    /// Return the inner vec
    ///
    /// ```
    /// use shared::maybe_err::ResultVec;
    ///
    /// fn call(input: ResultVec<i32, String>) {
    ///     for item in input.into_inner().into_iter() {
    ///         eprintln!("Got {:?}", item);
    ///     }
    /// }
    /// ```
    pub fn into_inner(self) -> Vec<Result<T, E>> {
        self.inner
    }

    /// Iterate over the results by reference, without consuming the `ResultVec`.
    pub fn iter(&self) -> std::slice::Iter<'_, Result<T, E>> {
        self.inner.iter()
    }

    /// Return the `Ok` values, draining the errors into the provided sink.
    ///
    /// Errors are converted via [`Into`] as they are moved into `errors`, so the sink can
    /// collect a different (e.g. boxed or stringified) error type. Use this when you want to
    /// keep going with the successful values while accumulating failures somewhere else.
    ///
    /// ```
    /// use shared::maybe_err::ResultVec;
    ///
    /// let results = vec![Ok(1), Err("bad"), Ok(2), Err("worse")]
    ///     .into_iter()
    ///     .collect::<ResultVec<_, _>>();
    ///
    /// let mut errors: Vec<String> = Vec::new();
    /// let oks = results.unwrap_drain_errs(&mut errors);
    ///
    /// assert_eq!(oks, vec![1, 2]);
    /// assert_eq!(errors, vec!["bad".to_string(), "worse".to_string()]);
    /// ```
    ///
    /// Named after the [`Result::unwrap_or`] / [`Result::unwrap_or_else`] family: `unwrap` returns
    /// the success values (`Vec<T>`) and the suffix names the error strategy — draining each `Err`
    /// into `sink` rather than substituting a fallback value.
    #[must_use]
    pub fn unwrap_drain_errs<X, Item>(self, sink: &mut X) -> Vec<T>
    where
        X: Extend<Item>,
        E: Into<Item>,
    {
        let mut oks = Vec::with_capacity(self.inner.len());

        for item in self.inner {
            match item {
                Ok(ok) => oks.push(ok),
                Err(err) => sink.extend(std::iter::once(err.into())),
            }
        }

        oks
    }
}

impl<T, E> FromIterator<Result<T, E>> for ResultVec<T, E> {
    fn from_iter<I: IntoIterator<Item = Result<T, E>>>(iter: I) -> Self {
        Self {
            inner: iter.into_iter().collect(),
        }
    }
}

impl<T, E> IntoIterator for ResultVec<T, E> {
    type Item = Result<T, E>;

    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.inner.into_iter()
    }
}

impl<'a, T, E> IntoIterator for &'a ResultVec<T, E> {
    type Item = &'a Result<T, E>;

    type IntoIter = std::slice::Iter<'a, Result<T, E>>;

    fn into_iter(self) -> Self::IntoIter {
        self.inner.iter()
    }
}

impl<T, E> From<Vec<Result<T, E>>> for ResultVec<T, E> {
    fn from(inner: Vec<Result<T, E>>) -> Self {
        Self { inner }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    #[test]
    fn unwrap_drain_errs_returns_oks_in_order_and_fills_sink() {
        let results = vec![Ok(1), Err("bad"), Ok(2), Err("worse"), Ok(3)]
            .into_iter()
            .collect::<ResultVec<_, _>>();
        let mut errors: Vec<String> = Vec::new();
        let oks = results.unwrap_drain_errs(&mut errors);
        assert_eq!(oks, vec![1, 2, 3]);
        assert_eq!(errors, vec!["bad".to_string(), "worse".to_string()]);
    }

    #[test]
    fn unwrap_drain_errs_handles_empty() {
        let results: ResultVec<i32, &str> = Vec::new().into();
        let mut errors: Vec<String> = Vec::new();
        let oks = results.unwrap_drain_errs(&mut errors);
        assert!(oks.is_empty());
        assert!(errors.is_empty());
    }

    #[test]
    fn from_vec_and_into_inner_round_trip() {
        let original: Vec<Result<i32, &str>> = vec![Ok(1), Err("bad"), Ok(2)];
        let round_tripped = ResultVec::from(original.clone()).into_inner();
        assert_eq!(round_tripped, original);
    }
}
