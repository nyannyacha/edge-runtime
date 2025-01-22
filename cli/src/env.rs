use base::runtime;
use clap::builder::BoolishValueParser;
use clap::builder::TypedValueParser;
use once_cell::sync::OnceCell;

pub(super) fn resolve_deno_runtime_env() {
  let boolish_parser = BoolishValueParser::new();
  let dumb_command = clap::Command::new(env!("CARGO_BIN_NAME"));
  let resolve_boolish_env =
    move |key: &'static str, cell: &'static OnceCell<bool>| {
      cell.get_or_init(|| {
        std::env::var_os(key)
          .map(|it| {
            boolish_parser
              .parse_ref(&dumb_command, None, &it)
              .unwrap_or_default()
          })
          .unwrap_or_default()
      })
    };

  runtime::MAYBE_DENO_VERSION.get_or_init(|| deno::version().to_string());

  resolve_boolish_env(
    "DENO_NO_DEPRECATION_WARNINGS",
    &runtime::SHOULD_DISABLE_DEPRECATED_API_WARNING,
  );

  resolve_boolish_env(
    "DENO_VERBOSE_WARNINGS",
    &runtime::SHOULD_USE_VERBOSE_DEPRECATED_API_WARNING,
  );

  resolve_boolish_env(
    "EDGE_RUNTIME_INCLUDE_MALLOCED_MEMORY_ON_MEMCHECK",
    &runtime::SHOULD_INCLUDE_MALLOCED_MEMORY_ON_MEMCHECK,
  );
}
