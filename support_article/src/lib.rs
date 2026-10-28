//! Generates the Ruby and JRuby version tables in the Dev Center article "Heroku Ruby Support".

use minijinja::{Environment, UndefinedBehavior, Value};
use tokio::task::JoinSet;

pub mod ruby_table;

#[derive(Debug, thiserror::Error)]
pub enum CheckError {
    #[error(transparent)]
    S3(#[from] shared::Error),

    #[error("S3 check task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
}

/// Every task's value, or the first error.
async fn join_all<T: 'static>(
    mut set: JoinSet<Result<T, CheckError>>,
) -> Result<Vec<T>, CheckError> {
    let mut values = Vec::new();
    while let Some(joined) = set.join_next().await {
        values.push(joined??);
    }
    Ok(values)
}

/// Undefined values are errors, so a misspelled variable fails instead of rendering blank.
pub(crate) fn render_template(
    name: &str,
    source: &str,
    context: Value,
) -> Result<String, minijinja::Error> {
    let mut environment = Environment::new();
    environment.set_undefined_behavior(UndefinedBehavior::Strict);
    environment.add_template(name, source)?;
    environment.get_template(name)?.render(context)
}

#[cfg(test)]
mod tests {
    use super::*;
    use minijinja::context;
    use pretty_assertions::assert_eq;

    #[test]
    fn render_template_errors_on_a_misspelled_attribute() {
        let error = render_template(
            "t",
            "{{ row.nmae }}",
            context! { row => context! { name => "x" } },
        )
        .unwrap_err();
        assert_eq!(error.kind(), minijinja::ErrorKind::UndefinedError);
    }
}
