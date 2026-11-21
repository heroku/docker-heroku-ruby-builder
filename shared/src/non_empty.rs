//! A collection guaranteed to contain at least one element

use std::{
    convert::TryFrom,
    fmt::Display,
    ops::{Deref, DerefMut},
};

/// A collection guaranteed to contain at least one element
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NonEmpty<T>(Vec<T>);

impl<T> NonEmpty<T> {
    pub fn new(first: T) -> Self {
        Self(vec![first])
    }

    pub fn push(&mut self, item: T) {
        self.0.push(item);
    }

    pub fn first_mut(&mut self) -> &mut T {
        self.0.first_mut().expect("non-empty")
    }

    pub fn split_first(&self) -> (&T, &[T]) {
        self.0.split_first().expect("non-empty")
    }

    #[allow(dead_code)]
    pub fn first(&self) -> &T {
        self.0.first().expect("non-empty")
    }

    pub fn last(&self) -> &T {
        self.0.last().expect("non-empty")
    }
}

impl<T> Deref for NonEmpty<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        &self.0
    }
}

// Safe because it returns an &[T] the inner can be manipulated
// but it's not like a `&mut Vec<T>` where someone could call pop().
impl<T> DerefMut for NonEmpty<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        &mut self.0
    }
}

impl<T> Extend<T> for NonEmpty<T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        self.0.extend(iter);
    }
}

impl<'a, T> IntoIterator for &'a NonEmpty<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

#[derive(Debug)]
pub struct NonEmptyError;
impl Display for NonEmptyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cannot construct a NonEmpty iterator from an empty one")
    }
}

impl<T> TryFrom<Vec<T>> for NonEmpty<T> {
    type Error = NonEmptyError;

    fn try_from(value: Vec<T>) -> Result<Self, Self::Error> {
        if value.is_empty() {
            Err(NonEmptyError)
        } else {
            Ok(NonEmpty(value))
        }
    }
}
