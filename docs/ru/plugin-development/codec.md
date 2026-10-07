# fairway-codec

Codec превращает целый буфер байтов в значение Rust и обратно.

Поддерживаются [текст](#значения-и-типы), [JSON](#json-и-toml), [TOML](#json-и-toml), [YAML](#yaml), [Markdown](#markdown) и [сжатие Zstd](#zstd).

- `Decode::decode` разбирает байты в значение указанного типа;
- `Encode::encode` превращает значение в байты.
Обе операции синхронные и работают с буфером целиком.

Зависимости плагина:

```toml
[dependencies]
fairway-codec.workspace = true
anyhow.workspace = true
```

## Быстрый старт

```rust
use fairway_codec::serde::Serialize;
use fairway_codec::{Encode, Error, Json};

#[derive(Serialize)]
#[serde(crate = "fairway_codec::serde")]
struct Settings {
    name: String,
    retries: u32,
}

fn settings_json() -> Result<Vec<u8>, Error> {
    let settings = Settings {
        name: "example".to_owned(),
        retries: 3,
    };

    Json(settings).encode()
}
```

Результат:

```json
{"name":"example","retries":3}
```

`Settings` — обычная Rust-структура. `#[derive(Serialize)]` добавляет
сериализацию её полей, а `Json(settings).encode()` превращает значение
структуры в байты JSON с переводом строки в конце.

## Значения и типы

Формат определяется типом: `Json<Vec<String>>` — это JSON-массив строк, а
`Toml<Dataset>` — TOML-документ со структурой `Dataset`.

| Тип | Разбор | Кодирование |
|---|---|---|
| `Vec<u8>` | байты как есть | байты как есть |
| `String` | проверяет UTF-8 без копирования | байты строки без копирования |
| `[u8; N]` | — | копирует массив |
| `Json<T>` | один JSON-документ | компактный JSON и перевод строки |
| `Toml<T>` | документ TOML | TOML без исходных комментариев и оформления |
| `Yaml<T>` | один YAML-документ | YAML без исходных комментариев и оформления |
| `Markdown` | проверяет UTF-8 и хранит текст | исходный текст байт в байт |
| `Zstd<T>` | распаковывает байты и разбирает их как `T` | кодирует `T` и сжимает его байты |

Для `Json<T>`, `Toml<T>` и `Yaml<T>` тип `T` должен реализовывать
`Deserialize` для чтения и `Serialize` для записи.

Трейты и derive-макросы доступны через `fairway_codec::serde`.
Атрибут `#[serde(crate = "fairway_codec::serde")]` указывает этот же
реэкспорт для сгенерированного кода.

Если структура данных известна, опишите её типом:

```rust
use fairway_codec::serde::{Deserialize, Serialize};
use fairway_codec::{Decode, Encode, Error, Toml};

#[derive(Deserialize, Serialize)]
#[serde(crate = "fairway_codec::serde")]
struct Dataset {
    name: String,
    files: Vec<String>,
}

fn main() -> Result<(), Error> {
    let input = br#"name = "events"
files = ["part-01.jsonl"]
"#
    .to_vec();

    let Toml(mut dataset) = Toml::<Dataset>::decode(input)?;
    dataset.files.push("part-02.jsonl".to_owned());

    let output = Toml(dataset).encode()?;
    print!("{}", String::decode(output)?);
    Ok(())
}
```

TOML разбирается в `Dataset`, затем в поле `files` добавляется ещё один файл.
После кодирования результат выглядит так:

```toml
name = "events"
files = ["part-01.jsonl", "part-02.jsonl"]
```

Методы забирают вход во владение и могут переиспользовать его память вместо
копирования. Чтобы закодировать значение, сохранив его у вызывающего кода,
передайте ссылку: `Json(&value).encode()`, `Toml(&value).encode()` или
`Yaml(&value).encode()`.

## Данные без фиксированной структуры

Если структура данных определяется во время выполнения, используйте
универсальные значения. Без параметра типа `Json` и `Yaml` используют
`json::Value`, а `Toml` — `toml::Value`.

Макросы `json!` и `toml!` создают такие значения без прямой зависимости от
`serde_json` и `toml`:

```rust
use fairway_codec::{Decode, Encode, Error, Json, json};

fn main() -> Result<(), Error> {
    let input = br#"{"name":"example","ready":false}"#.to_vec();
    let Json(mut document): Json = Json::decode(input)?;
    document["ready"] = json!(true);

    let output = Json(document).encode()?;
    print!("{}", String::decode(output)?);
    Ok(())
}
```

`document` имеет тип `json::Value`: поля доступны по строковым ключам.
Здесь меняется поле `ready`, а `name` сохраняется:

```json
{"name":"example","ready":true}
```

Универсальное значение `Yaml` подходит для данных, представимых в JSON.
Для других типов YAML, например словаря с составными ключами, укажите
конкретный `T`.

## Откуда берутся данные

Байты готовит вызывающий код. Кодек выполняет только преобразование в памяти,
без ввода-вывода.

Например, [fairway-fs](fs.md) читает и записывает файлы через те же трейты.
Для работы с ним тип должен быть `Send + 'static`, а его ошибка — `Send` и
преобразовываться в `Box<dyn Error + Send + Sync>`. Этим условиям отвечают,
например, `anyhow::Error`, `io::Error`, `codec::Error` и `Infallible`.

Напрямую преобразование выполняется в вызывающем потоке. При чтении и записи
файлов `fairway-fs` запускает его в отдельном потоке блокирующих операций.
Если вы разбираете большие данные прямо в асинхронной задаче, вынесите работу
в `tokio::task::spawn_blocking`, чтобы не задерживать другие задачи.

## JSON и TOML

JSON кодируется компактно, с переводом строки в конце. При разборе после
значения допустимы только пробельные символы, а некорректный UTF-8 отклоняется
во всём входе, даже в полях, которые тип пропускает.

Верхний уровень документа TOML — всегда таблица, поэтому `T` обычно структура
или словарь.

При кодировании TOML теряются комментарии, пустые строки и исходное
оформление. Чтобы сохранить их, меняйте документ как `String`.

## YAML

`Yaml<T>` читает и записывает один YAML-документ. Например, так можно изменить
имя в метаданных:

```rust
use fairway_codec::serde::{Deserialize, Serialize};
use fairway_codec::{Decode, Encode, Error, Yaml};

#[derive(Deserialize, Serialize)]
#[serde(crate = "fairway_codec::serde")]
struct Metadata {
    name: String,
    description: String,
}

fn main() -> Result<(), Error> {
    let input = b"name: draft\ndescription: Check changes.\n".to_vec();
    let Yaml(mut metadata) = Yaml::<Metadata>::decode(input)?;
    metadata.name = "review".to_owned();

    let output = Yaml(metadata).encode()?;
    print!("{}", String::decode(output)?);
    Ok(())
}
```

После разбора меняется поле `name` Rust-структуры, а `description`
сохраняется. Результат кодирования:

```yaml
name: review
description: Check changes.
```

Вход целиком проверяется на UTF-8. Несколько документов в одном буфере и
повторяющиеся ключи словаря считаются ошибками.

Логическими значениями распознаются только `true` и `false`. Значения `yes`,
`no`, `on` и `off` остаются строками.

Кодирование сохраняет значения, но не исходные комментарии и оформление. Чтобы
сохранить текст документа, меняйте его как `String`.

`Yaml` работает с самим YAML. Выделение заголовка между строками `---` из
Markdown и проверку полей скилла выполняет код соответствующего формата.

## Markdown

`Markdown` хранит исходный текст документа. `events()` разбирает его в
последовательность событий: начало и конец заголовка, абзаца, списка,
фрагменты текста и так далее. Типы событий находятся в `codec::markdown`, это
типы [pulldown-cmark](https://docs.rs/pulldown-cmark).

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
    let source = "# Intro\n\nText.\n\n## Details\n\n# Use `fairway`\n";
    let document = Markdown::parse(source.to_owned());
    let headings = titles(&document);

    assert_eq!(headings, ["Intro", "Use fairway"]);
    println!("{headings:?}");
}
```

Из документа извлекаются только заголовки первого уровня. Встроенный код
включается в текст заголовка, а заголовок `Details` второго уровня пропускается:

```text
["Intro", "Use fairway"]
```

Поддерживается CommonMark без расширений: таблицы, сноски, зачёркивание,
списки задач и front matter не распознаются.

`encode` возвращает исходный текст байт в байт, поэтому разбор и кодирование
никогда не меняют документ.

Каждый вызов `events()` разбирает документ заново, синхронно в вызывающем
потоке. Если события нужны несколько раз, соберите их в вектор.

События по возможности заимствуют текст документа. Чтобы сохранить их дольше,
чем живёт документ, преобразуйте их через `Event::into_static`. События
доступны только для чтения. Чтобы изменить документ, соберите новый текст и
оберните его через `Markdown::parse`.

## Zstd

`Zstd<T>` — обёртка над обычным значением типа `T`. Кодирование вызывает
`T::encode()` и сжимает полученные байты на уровне 3. Декодирование
распаковывает вход и передаёт весь полученный буфер в `T::decode()`.

Без параметра типа `Zstd` использует `Vec<u8>` для произвольных байтов. Для
текста используйте `Zstd<String>`, для структурированных данных — обёртку над
соответствующим кодеком, например `Zstd<Json<T>>`:

```rust
use fairway_codec::serde::{Deserialize, Serialize};
use fairway_codec::{Decode, Encode, Error, Json, Zstd};

#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(crate = "fairway_codec::serde")]
struct Settings {
    name: String,
    retries: u32,
}

fn main() -> Result<(), Error> {
    let settings = Settings {
        name: "example".to_owned(),
        retries: 3,
    };

    let packed = Zstd(Json(&settings)).encode()?;
    let Zstd(Json(restored)) = Zstd::<Json<Settings>>::decode(packed)?;

    assert_eq!(restored, settings);
    println!("{restored:?}");
    Ok(())
}
```

При записи `Settings` превращается в JSON, затем в сжатые байты.
При чтении выполняются обратные преобразования, и результат снова имеет
тип `Settings`:

```text
Settings { name: "example", retries: 3 }
```

`Json(&settings)` заимствует исходную структуру, поэтому она остаётся
доступной после кодирования.

Для кодирования нужен `T: Encode`, для декодирования — `T: Decode`. Второе
направление не обязательно. Ошибка вложенного кодека должна преобразовываться
в `Box<dyn std::error::Error + Send + Sync>`, как ошибки встроенных кодеков,
`io::Error`, `anyhow::Error` и `Infallible`.

Результат — стандартный Zstd-фрейм. Заголовка Fairway с указанием внутреннего
формата нет: тип выбирает вызывающий код при декодировании. Последовательные
фреймы распаковываются в один буфер до вызова вложенного декодера.

Оба преобразования работают с целыми буферами в памяти. Дополнительного
ограничения на размер распакованных данных нет.

`into_inner()` возвращает вложенное значение, `as_ref()` — ссылку на него.
Сама обёртка хранит обычное значение, а не его сжатое представление.

## Собственный формат

Для собственного формата реализуйте `Decode` и `Encode` для своего типа.
Внутри удобно пользоваться встроенными кодеками.

Например, точку `Point` можно хранить как два целых числа через запятую.
Декодер превращает байты `12,34` в `Point { x: 12, y: 34 }`, а кодер
превращает структуру обратно в байты:

```rust
use std::convert::Infallible;

use anyhow::Context;
use fairway_codec::{Decode, Encode};

#[derive(Debug, PartialEq)]
struct Point {
    x: i32,
    y: i32,
}

impl Decode for Point {
    type Error = anyhow::Error;

    fn decode(bytes: Vec<u8>) -> anyhow::Result<Self> {
        let text = String::decode(bytes)?;
        let (x, y) = text.split_once(',').context("expected x,y")?;
        Ok(Self {
            x: x.parse().context("invalid x coordinate")?,
            y: y.parse().context("invalid y coordinate")?,
        })
    }
}

impl Encode for Point {
    type Error = Infallible;

    fn encode(self) -> Result<Vec<u8>, Self::Error> {
        format!("{},{}", self.x, self.y).encode()
    }
}

fn main() -> anyhow::Result<()> {
    let point = Point::decode(b"12,34".to_vec())?;
    assert_eq!(point, Point { x: 12, y: 34 });

    let Ok(bytes) = Point { x: -5, y: 8 }.encode();
    assert_eq!(bytes, b"-5,8");
    Ok(())
}
```

`decode` и `encode` должны выполнять только преобразование, без ввода-вывода.
Это позволяет использовать кодек в разных потребителях. В частности, в `write`
и `edit` `fairway-fs` вызывает его, удерживая блокировку файла.

## Ошибки

Встроенные кодеки, которые могут завершиться ошибкой, возвращают
`fairway_codec::Error`. В сообщении указан формат и, если место ошибки
известно, строка и столбец:

```text
cannot decode json at line 3, column 2: expected value at line 3 column 2
```

Для обработки в коде доступны варианты ошибки и их поля:

| Вариант | Что произошло |
|---|---|
| `Decode { format, line, column, source }` | Данные не удалось разобрать |
| `Encode { format, source }` | Значение не удалось закодировать |

`format` — это `"text"` (для `String`), `"json"`, `"toml"`, `"yaml"`,
`"markdown"` или `"zstd"`. Номера строки и столбца начинаются с единицы;
столбец считается в байтах. Для некорректного UTF-8 это позиция первого
неверного байта.

Исходная ошибка доступна через `std::error::Error::source()`. `Error`
реализует `std::error::Error + Send + Sync` и владеет своей диагностикой.

Типы исходных ошибок YAML доступны как `codec::yaml::DecodeError` и
`codec::yaml::EncodeError`; зависимость от YAML-парсера плагину не нужна.

У ошибки `Zstd` формат всегда `"zstd"`, без строки и столбца. Её `source()`
сохраняет ошибку сжатия или вложенного кодека, включая позицию, указанную
вложенным декодером.

В собственном кодеке тип ошибки выбираете вы. В примере выше
`Point::decode` использует `anyhow::Error`, а `Point::encode` — `Infallible`.
Форматы, которые не могут завершиться ошибкой, используют `Infallible`.
Результат такого преобразования можно получить без `unwrap`:

```rust
use fairway_codec::Encode;

let Ok(bytes) = "text".to_owned().encode();
assert_eq!(bytes, b"text");
```

Когда файл читают и записывают через `fairway-fs`, ошибка разбора становится
источником `fs::Error` вида `InvalidData`, а ошибка кодирования — вида
`InvalidInput`. Паника в вызванном кодеке продолжается в задаче, которая ждёт
`read`, `write` или `edit`, а файл остаётся прежним.

При ошибке частичный результат преобразования не возвращается.
