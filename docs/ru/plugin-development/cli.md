# Команды

`fairway-cli` позволяет плагину объявлять команды и аргументы CLI.

## Быстрый старт

`Namespace` объединяет команды под общим именем в CLI: `fairway <namespace>`.

`namespace!(CLI, "name", "about")` объявляет пространство: `CLI` — имя объекта
в Rust-коде, `"name"` — имя в CLI, `"about"` — описание для справки.

`command!(CLI, "about", handler)` связывает вызов `fairway <namespace>`
с обработчиком.

```rust
// src/lib.rs

fairway_cli::namespace!(CLI, "hello", "Print a greeting");

async fn hello() -> anyhow::Result<()> {
    println!("Hello, world!");
    Ok(())
}

fairway_cli::command!(CLI, "Print a greeting", hello);
```

```console
$ fairway hello
Hello, world!

```

## Аргументы и подкоманды

Для подкоманды в `command!` добавляется её имя:
`command!(CLI, "name", "about", handler)` регистрирует вызов
`fairway <namespace> <name>`.

Аргументы CLI описываются типом с `clap::Args`;
обработчик получает значение этого типа.

```rust
// src/lib.rs

fairway_cli::namespace!(CLI, "text", "Text commands");

#[derive(clap::Args)]
struct Upper {
    /// Text to convert to uppercase.
    text: String,
}

#[derive(clap::Args)]
struct Repeat {
    /// Text to repeat.
    text: String,

    /// Number of repetitions.
    #[arg(long, default_value_t = 2)]
    times: usize,
}

async fn upper(args: Upper) -> anyhow::Result<()> {
    println!("{}", args.text.to_uppercase());
    Ok(())
}

async fn repeat(args: Repeat) -> anyhow::Result<()> {
    for _ in 0..args.times {
        println!("{}", args.text);
    }
    Ok(())
}

fairway_cli::command!(CLI, "upper", "Convert text to uppercase", upper);
fairway_cli::command!(CLI, "repeat", "Repeat text", repeat);
```

```console
$ fairway text upper "Hello, world!"
HELLO, WORLD!

$ fairway text repeat hello --times 3
hello
hello
hello

```

## Потоки и завершение работы

Обработчик может дополнительно принимать `Shutdown` — уведомление
о запросе завершения. `requested().await` ожидает этот запрос.

`workers = N` задаёт число рабочих потоков Tokio; `0` — по доступным ядрам.
Для выбора из аргументов передайте функцию: `workers = |args: &Serve| args.workers`.
Fairway вычисляет число потоков перед запуском обработчика.

```rust
// src/lib.rs

use std::net::SocketAddr;

use axum::{Router, routing::get};
use fairway_cli::Shutdown;
use tokio::net::TcpListener;

fairway_cli::namespace!(CLI, "http", "HTTP server");

#[derive(clap::Args)]
struct Serve {
    /// Address to listen on.
    #[arg(long, default_value = "127.0.0.1:8080")]
    listen: SocketAddr,

    /// Worker threads; zero uses the available cores.
    #[arg(long, default_value_t = 0)]
    workers: usize,
}

async fn serve(args: Serve, shutdown: Shutdown) -> anyhow::Result<()> {
    let listener = TcpListener::bind(args.listen).await?;
    let app = Router::new().route("/health", get(|| async { "ok" }));

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown.requested().await;
        })
        .await?;

    Ok(())
}

fairway_cli::command!(CLI, "serve", "Start the server", serve, workers = |args: &Serve| args.workers);
```

```console,ignore
$ fairway http serve --listen 127.0.0.1:9000 --workers 4
```

## API

### Регистрация

- `namespace!(CLI, "name", "about")` — объявляет
  `pub(crate) static CLI: Namespace` для `fairway name`.
- `command!(CLI, "name", "about", handler)` — добавляет подкоманду.
- `command!(CLI, "about", handler)` — задаёт обработчик для `fairway <namespace>`.

Имя пространства уникально в приложении, имя команды — внутри пространства.
В пространстве регистрируются либо подкоманды, либо обработчик самого пространства.
Повторы имён проверяет общий тест приложения: `cargo test -p fairway`.
Проверка охватывает плагины и Cargo features, подключённые в этой сборке;
`cargo build` сам её не запускает. Смешение двух видов команд и повторный
обработчик самого пространства — ошибки компиляции.
Имена непустые, без пробелов и управляющих символов.

### Справка

`fairway-cli` автоматически формирует справку по `--help` для приложения,
пространства команд и отдельной подкоманды.

`about` задаёт описание в справке, doc-комментарии полей аргументов —
описания параметров CLI.

### Обработчики

Обработчик — `async fn` с результатом `anyhow::Result<()>` и `Send`-future.
Допустимые параметры:

- `()`.
- `(args: T)`.
- `(shutdown: Shutdown)`.
- `(args: T, shutdown: Shutdown)`.

`T` реализует `clap::Args`. Совместимость обработчика проверяется при компиляции.

### Потоки

`command!(…, workers = N)` задаёт число рабочих потоков Tokio;
`0` — по доступным ядрам. Без `workers` используется однопоточное выполнение.
`workers = |args: &T| args.workers` выбирает число из разобранных аргументов.
Этот параметр не ограничивает отдельные потоки, созданные самим плагином.

### Завершение

`Shutdown` уведомляет о запросе завершения:

- `requested().await` — ждёт запрос; возвращается сразу, если он уже получен.
- `is_requested()` — проверяет наличие запроса.
- `clone()` — создаёт копию, наблюдающую тот же запрос.
- `new()` — создаёт объект без источника запроса, для тестов.

Fairway обрабатывает сигналы и даёт команде время на завершение:
по умолчанию 15 секунд, срок настраивается на уровне Fairway.
После истечения срока процесс принудительно завершается, даже если
обработчик не использует `Shutdown` или блокирует выполнение.

### Ошибки

Возвращайте ошибку через `Result`: Fairway напечатает её в stderr и установит
код выхода. Обработчик не должен повторно печатать ошибку или вызывать `exit`.

- `0` — `Ok(())`, справка или версия.
- `1` — ошибка команды, подготовки или превышение времени завершения.
- `2` — ошибка аргументов CLI.
