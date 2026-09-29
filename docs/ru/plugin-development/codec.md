# fairway-codec

Преобразования между целыми буферами байтов и значениями Rust.

Крейт превращает байты в значения Rust и обратно: текст, JSON, TOML
и Markdown. Откуда берутся байты, он не знает: источники и приёмники данных
реализуют другие крейты. Например, [fairway-fs](fs.md) читает и записывает
файлы в тех же форматах. Напрямую `fairway-codec` нужен для данных в памяти,
например полученных по сети, и для собственных форматов.

Зависимости плагина:

```toml
[dependencies]
fairway-codec.workspace = true
```

## Быстрый старт

`Decode::decode` разбирает байты в значение указанного типа, `Encode::encode`
превращает значение в байты. Формат определяется типом: `Json<Vec<String>>` —
это JSON-массив строк.

```rust
use fairway_codec::{self as codec, Decode, Encode, Json};

fn add_word(input: Vec<u8>) -> Result<Vec<u8>, codec::Error> {
    let Json(mut words): Json<Vec<String>> = Json::decode(input)?;
    words.push("mouse".to_owned());
    Json(words).encode()
}
```

- Оба метода работают с буфером целиком.
- Вход они забирают во владение и могут переиспользовать его память вместо
  копирования. Если исходные данные ещё понадобятся, сделайте копию заранее.
- Преобразование синхронное и выполняется в вызывающем потоке. Когда файл
  читает или записывает `fairway-fs`, преобразование выполняется в отдельном
  потоке блокирующих операций. Если вы разбираете большие данные прямо
  в асинхронной задаче, вынесите работу в `tokio::task::spawn_blocking`, чтобы
  не задерживать другие задачи.

## Встроенные форматы

| Тип | Разбор | Кодирование |
|---|---|---|
| `Vec<u8>` | байты как есть | байты как есть |
| `String` | проверяет UTF-8 без копирования | байты строки без копирования |
| `[u8; N]` | — | копирует массив |
| `Json<T>` | один JSON-документ | компактный JSON и перевод строки |
| `Toml<T>` | документ TOML | TOML без исходных комментариев и оформления |
| `Markdown` | проверяет UTF-8 и хранит текст | исходный текст байт в байт |

## JSON и TOML

`Json<T>` и `Toml<T>` — обёртки над значением, которое реализует `serde`.
Если структура данных известна, опишите её типом:

```rust
use fairway_codec::{self as codec, Decode, Encode, Toml};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
struct Dataset {
    name: String,
    files: Vec<String>,
}

fn add_part(input: Vec<u8>) -> Result<Vec<u8>, codec::Error> {
    let Toml(mut dataset): Toml<Dataset> = Toml::decode(input)?;
    dataset.files.push("part-02.jsonl".to_owned());
    Toml(dataset).encode()
}
```

Без параметра используется универсальное значение `json::Value`
или `toml::Value`. Макросы `json!` и `toml!` создают такие значения
без прямой зависимости от `serde_json` и `toml`.

```rust
use fairway_codec::{self as codec, Decode, Encode, Json, json};

fn version(input: Vec<u8>) -> Result<Option<String>, codec::Error> {
    let Json(document): Json = Json::decode(input)?;
    Ok(document["version"].as_str().map(str::to_owned))
}

fn status(name: &str, ready: bool) -> Result<Vec<u8>, codec::Error> {
    Json(json!({ "name": name, "ready": ready })).encode()
}
```

- JSON кодируется компактно, с переводом строки в конце. При разборе после
  значения допустимы только пробельные символы, а некорректный UTF-8
  отклоняется во всём входе, даже в полях, которые тип пропускает.
- Верхний уровень документа TOML — всегда таблица, поэтому `T` обычно
  структура или словарь.
- При кодировании TOML теряются комментарии, пустые строки и исходное
  оформление. Чтобы сохранить их, меняйте документ как `String`.
- Чтобы закодировать значение, не отдавая его, передайте ссылку:
  `Json(&value).encode()`.

## Markdown

`Markdown` хранит исходный текст документа. `events()` разбирает его
в последовательность событий: начало и конец заголовка, абзаца, списка,
фрагменты текста и так далее. Типы событий находятся в `codec::markdown`,
это типы [pulldown-cmark](https://docs.rs/pulldown-cmark).

```rust
use fairway_codec::{Markdown, markdown::{Event, HeadingLevel, Tag, TagEnd}};

fn titles(document: &Markdown) -> Vec<String> {
    let mut titles = Vec::new();
    let mut current = None;
    for event in document.events() {
        match event {
            Event::Start(Tag::Heading { level: HeadingLevel::H1, .. }) => {
                current = Some(String::new());
            }
            Event::Text(text) | Event::Code(text) => {
                if let Some(title) = &mut current {
                    title.push_str(&text);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some(title) = &mut current {
                    title.push(' ');
                }
            }
            Event::End(TagEnd::Heading(HeadingLevel::H1)) => {
                titles.extend(current.take());
            }
            _ => {}
        }
    }
    titles
}

fn main() {
    let document = Markdown::parse("# Intro\n\nText.\n\n# Use `fairway`\n".to_owned());
    assert_eq!(titles(&document), ["Intro", "Use fairway"]);

    for source in ["First\nsecond\n===\n", "First  \nsecond\n===\n"] {
        let document = Markdown::parse(source.to_owned());
        assert_eq!(titles(&document), ["First second"]);
    }
}
```

- Поддерживается CommonMark без расширений: таблицы, сноски, зачёркивание,
  списки задач и front matter не распознаются.
- `encode` возвращает исходный текст байт в байт, поэтому разбор
  и кодирование никогда не меняют документ.
- Каждый вызов `events()` разбирает документ заново, синхронно в вызывающем
  потоке. Если события нужны несколько раз, соберите их в вектор.
- События по возможности заимствуют текст документа. Чтобы сохранить их
  дольше, чем живёт документ, преобразуйте их через `Event::into_static`.
- События доступны только для чтения. Чтобы изменить документ, соберите
  новый текст и оберните его через `Markdown::parse`.

## Ошибки

Встроенные форматы, которые могут не справиться с данными, возвращают
`codec::Error`:

- `Decode { format, line, column, source }` — данные не удалось разобрать;
- `Encode { format, source }` — значение не удалось закодировать.

`format` — это `"text"` (для `String`), `"json"`, `"toml"` или `"markdown"`.
`line` и `column` отсчитываются от 1 и заполняются, если позиция известна;
столбец считается в байтах. Для некорректного UTF-8 это позиция первого
неверного байта. Ошибка парсера доступна в `source`.

Форматы, которые не могут завершиться ошибкой, используют тип ошибки
`Infallible`. Результат такого преобразования можно получить без `unwrap`:

```rust
use fairway_codec::Encode;

let Ok(bytes) = "text".to_owned().encode();
assert_eq!(bytes, b"text");
```

Когда файл читают и записывают через `fairway-fs`, ошибка разбора становится
источником `fs::Error` вида `InvalidData`, а ошибка кодирования — вида
`InvalidInput`.

## Собственный формат

Чтобы преобразовывать свой формат и использовать его в `fairway-fs`,
реализуйте для своего типа `Decode` и `Encode`. Внутри удобно пользоваться
встроенными форматами.

Пример: Markdown с обязательным заголовком TOML между строками `+++`.

```text
+++
title = "Dataset"
+++
# Description

The first part of the dataset.
```

```rust
use anyhow::Context;
use fairway_codec::{Decode, Encode, Markdown, Toml};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
struct Frontmatter {
    title: String,
}

struct Article {
    frontmatter: Frontmatter,
    body: Markdown,
}

impl Decode for Article {
    type Error = anyhow::Error;

    fn decode(bytes: Vec<u8>) -> anyhow::Result<Self> {
        let text = String::decode(bytes)?;
        let content = text
            .strip_prefix("+++\r\n")
            .or_else(|| text.strip_prefix("+++\n"))
            .context("the document must start with TOML frontmatter")?;
        let mut header_end = 0;
        for line in content.split_inclusive('\n') {
            if line.trim_end_matches(['\r', '\n']) == "+++" {
                let header = &content[..header_end];
                let body = &content[header_end + line.len()..];
                let Toml(frontmatter) = Toml::decode(header.as_bytes().to_vec())?;
                return Ok(Self {
                    frontmatter,
                    body: Markdown::parse(body.to_owned()),
                });
            }
            header_end += line.len();
        }
        anyhow::bail!("the closing +++ line is missing")
    }
}

impl Encode for Article {
    type Error = anyhow::Error;

    fn encode(self) -> anyhow::Result<Vec<u8>> {
        let mut bytes = b"+++\n".to_vec();
        bytes.extend(Toml(self.frontmatter).encode()?);
        bytes.extend_from_slice(b"+++\n");
        bytes.extend(self.body.encode()?);
        Ok(bytes)
    }
}

fn change_title(input: Vec<u8>, title: String) -> anyhow::Result<Vec<u8>> {
    let mut article = Article::decode(input)?;
    article.frontmatter.title = title;
    article.encode()
}

fn main() -> anyhow::Result<()> {
    for newline in ["\n", "\r\n"] {
        let body = format!("# Description{newline}{newline}Text.{newline}");
        let input = format!("+++{newline}title = \"Old\"{newline}+++{newline}{body}");
        let output = change_title(input.into_bytes(), "New".to_owned())?;
        let article = Article::decode(output)?;
        assert_eq!(article.frontmatter.title, "New");
        assert_eq!(article.body.as_ref(), body);
    }
    Ok(())
}
```

- `decode` и `encode` синхронные. Выполняйте в них только преобразование,
  без ввода-вывода: в `write` и `edit` `fairway-fs` вызывает их, удерживая
  блокировку файла.
- Тип ошибки выбираете вы. Встроенные форматы используют `codec::Error`,
  пример выше — `anyhow::Error`, а формат, который не может завершиться
  ошибкой, — `Infallible`.
- Чтобы тип работал с `fairway-fs`, он должен быть `Send + 'static`, а его
  ошибка — `Send` и преобразовываться в `Box<dyn Error + Send + Sync>`. Этим
  условиям отвечают, например, `anyhow::Error`, `io::Error`, `codec::Error`
  и `Infallible`.
- Паника в `decode` или `encode`, которые вызвал `fairway-fs`, продолжается
  в задаче, которая ждёт `read`, `write` или `edit`, а файл остаётся прежним.

## Документация API

Полное описание типов, трейтов и ошибок — в документации крейта. Локально
её открывает команда

```sh
cargo doc -p fairway-codec --open
```

Опубликованные версии доступны на [docs.rs](https://docs.rs/fairway-codec).
