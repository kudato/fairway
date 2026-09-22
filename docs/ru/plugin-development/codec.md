# Кодек

`fairway-codec` разбирает текст и байты в памяти и кодирует значения обратно в байты.

## Быстрый старт

`decode(data).await` разбирает данные в указанном типе, `encode(value).await`
кодирует значение в `Vec<u8>`. Тип значения задаёт формат: например,
`Json<Vec<String>>` — JSON-массив строк.

Обе функции выполняют преобразование в вычислительном пуле Fairway.
Входные данные передаются по значению.

```rust
use fairway_codec::{self as codec, Json};

async fn encode_words() -> Result<Vec<u8>, codec::Error> {
    let words: Json<Vec<String>> = codec::decode(r#"["кошка", "собака"]"#).await?;
    codec::encode(words).await
}
```

## Синхронное преобразование

Методы трейтов `Decode` и `Encode` выполняются на вызывающем потоке.
Они подходят для коротких операций и вызова одного кодека внутри другого.

```rust
use fairway_codec::{self as codec, Decode, Encode, Json};

fn encode_three_numbers() -> Result<Vec<u8>, codec::Error> {
    let numbers: Json<Vec<u64>> = Json::decode(b"[10, 20, 30]")?;
    Ok(numbers.encode()?.into_owned())
}
```

Длительные преобразования вызывайте через `codec::decode` или `codec::encode`.
Серия коротких синхронных вызовов также занимает поток до завершения всей серии.

## Форматы целого документа

### JSON и TOML

`Json<T>` и `Toml<T>` содержат разобранное значение `T`.
Метод `into_inner` извлекает его для изменения. Для обратного кодирования
оберните изменённое значение в `Json` или `Toml`.

Например, описание датасета в TOML:

```toml
name = "documents"
files = ["part-01.jsonl"]
```

```rust
use fairway_codec::{self as codec, Toml};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
struct Dataset {
    name: String,
    files: Vec<String>,
}

async fn add_part(input: String) -> Result<Vec<u8>, codec::Error> {
    let dataset: Toml<Dataset> = codec::decode(input).await?;
    let mut dataset = dataset.into_inner();
    dataset.files.push("part-02.jsonl".into());
    codec::encode(Toml(dataset)).await
}
```

### Markdown

`Markdown` хранит исходный текст документа. Метод `events` создаёт итератор
по его элементам: тексту, началу и концу заголовка, абзаца, списка и других
конструкций. Общий список событий не создаётся и не кешируется.
`encode` возвращает исходный текст без изменений и без разбора.

```rust
use fairway_codec::{self as codec, Markdown, markdown::Event};

async fn inspect_markdown(input: String) -> anyhow::Result<Vec<u8>> {
    let document: Markdown = codec::decode(input).await?;
    for event in document.events() {
        if let Event::Text(text) = event {
            println!("{text}");
        }
    }
    Ok(codec::encode(document).await?)
}
```

Кодирование побайтово сохраняет отступы, маркеры, экранирование и переносы строк.
Каждый вызов `events()` запускает новый разбор. Сам парсер выделяет рабочую
память: итератор не означает обработку с постоянным расходом памяти.

Создание и обход итератора синхронны и выполняются на вызывающем потоке.
`codec::decode::<Markdown>().await` только сохраняет текст после проверки UTF-8;
последующий обход не переносится в вычислительный пул автоматически.
Для больших документов выполняйте весь анализ внутри `Decode` собственного
типа и вызывайте его через `codec::decode`, как описано в разделе
[«Собственные типы»](#собственные-типы).

При необходимости сохраните события явно:

```rust
use fairway_codec::Markdown;

let document = Markdown::parse("# Заголовок\n");
let events: Vec<_> = document.events().collect();
assert_eq!(events.len(), 3);

// Такие события можно использовать и после удаления документа.
let owned: Vec<_> = document.events().map(|event| event.into_static()).collect();
drop(events);
drop(document);
assert_eq!(owned.len(), 3);
```

Это изменение API: раньше `events()` возвращал срез `&[Event<'static>]`,
теперь — итератор со значениями `Event<'_>`, которые могут заимствовать текст
документа. Для подсчёта используйте `.count()` вместо `.len()`, для индексации
или повторного использования готовых событий — явно собранный `Vec`.
Вызов `.iter()` перед обходом больше не нужен. Сбор полного списка вновь
требует памяти под все события.

## Потоковая обработка

### JSONL

`Jsonl<T>` разбирает и кодирует последовательность JSON-значений, по одному на строку.
Его методы синхронные. Для обработки партии записей в пуле их можно вызвать
внутри собственной реализации `Decode` или `Encode`.

#### Разбор JSONL

`push` принимает порцию байтов. После каждой порции вызывайте `next` до `None`,
чтобы получить все доступные записи.
Порция может заканчиваться внутри строки или многобайтового символа UTF-8.

Когда все байты переданы, вызовите `finish` и снова читайте через `next` до `None`.
Так будет разобрана и последняя строка без завершающего `\n`.

```rust
use fairway_codec::{self as codec, Jsonl};

fn main() -> Result<(), codec::Error> {
    let input = "\"кошка\"\n\"собака\"";
    let mut words = Jsonl::<String>::new();

    for chunk in input.as_bytes().chunks(5) {
        words.push(chunk)?;
        while let Some(word) = words.next()? {
            println!("{word}");
        }
    }

    words.finish();
    while let Some(word) = words.next()? {
        println!("{word}");
    }
    Ok(())
}
```

#### Кодирование в JSONL

`StreamEncode::encode` добавляет JSON-представление записи и завершающий `\n`
в выходной буфер. После передачи байтов потребителю буфер можно очистить
и использовать для следующей записи.

```rust
use fairway_codec::{self as codec, Jsonl, StreamEncode};

fn encode_words(words: &[String]) -> Result<Vec<u8>, codec::Error> {
    let mut encoder = Jsonl::<String>::new();
    let mut bytes = Vec::new();
    for word in words {
        encoder.encode(word, &mut bytes)?;
    }
    StreamEncode::finish(&mut encoder, &mut bytes)?;
    Ok(bytes)
}
```

`StreamEncode::finish` завершает кодирование. Отправка оставшихся байтов,
закрытие stdin или сохранение файла выполняются отдельно.

### Документ из частей

`Decoder<T>` и `Encoder<T>` подключают типы `Decode` и `Encode` к потоковому API:

- `Decoder<T>` принимает байты частями и возвращает один документ после `finish`.
- `Encoder<T>` принимает один документ и добавляет его представление в выходной буфер.

Поддерживаются `Json<T>`, `Toml<T>`, `Markdown`, текст, байты и собственные типы.
Документ должен помещаться в память. Для последовательности записей используется `Jsonl<T>`.

```rust
use anyhow::Context;
use fairway_codec::{Decoder, Encoder, Markdown, StreamDecode, StreamEncode};

fn markdown_from_chunks(chunks: &[&[u8]]) -> anyhow::Result<Vec<u8>> {
    let mut decoder = Decoder::<Markdown>::new();
    for chunk in chunks {
        decoder.push(chunk)?;
    }
    decoder.finish()?;
    let document = decoder.next()?.context("Декодер не вернул документ")?;

    let mut encoder = Encoder::<Markdown>::new();
    let mut bytes = Vec::new();
    encoder.encode(&document, &mut bytes)?;
    encoder.finish(&mut bytes)?;
    Ok(bytes)
}
```

## Собственные типы

`Decode` задаёт разбор байтов в собственный тип, `Encode` — обратное преобразование.
Трейты реализуются независимо. Вложенные форматы вызываются через синхронные методы:
всё преобразование выполняется в одной задаче при вызове `codec::decode` или `codec::encode`.

Собственный тип позволяет добавить к Markdown обязательный TOML frontmatter
с полем `title` между строками `+++`:

```text
+++
title = "Датасет"
+++
# Описание

Первая часть датасета.
```

```rust
use std::borrow::Cow;

use anyhow::Context;
use fairway_codec::{self as codec, Decode, Encode, Markdown, Toml};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
struct Frontmatter {
    title: String,
}

struct Article {
    frontmatter: Frontmatter,
    body: Markdown,
}

impl codec::Decode for Article {
    type Error = anyhow::Error;

    fn decode(bytes: &[u8]) -> anyhow::Result<Self> {
        let text = std::str::from_utf8(bytes)?.replace("\r\n", "\n");
        let content = text
            .strip_prefix("+++\n")
            .context("Документ должен начинаться с TOML frontmatter")?;
        let (header, body) = content
            .split_once("\n+++\n")
            .or_else(|| content.strip_suffix("\n+++").map(|header| (header, "")))
            .context("Отсутствует закрывающая строка +++")?;
        let frontmatter: Toml<Frontmatter> = Toml::decode(header.as_bytes())?;

        Ok(Self {
            frontmatter: frontmatter.into_inner(),
            body: Markdown::parse(body),
        })
    }
}

impl codec::Encode for Article {
    type Error = anyhow::Error;

    fn encode(&self) -> anyhow::Result<Cow<'_, [u8]>> {
        let header = Toml(&self.frontmatter).encode()?.into_owned();
        let mut bytes = b"+++\n".to_vec();
        bytes.extend_from_slice(&header);
        if !bytes.ends_with(b"\n") {
            bytes.push(b'\n');
        }
        bytes.extend_from_slice(b"+++\n");
        bytes.extend_from_slice(&self.body.encode()?);
        Ok(Cow::Owned(bytes))
    }
}

async fn change_title(input: String, title: String) -> anyhow::Result<Vec<u8>> {
    let mut article: Article = codec::decode(input).await?;
    article.frontmatter.title = title;
    codec::encode(article).await
}
```

Плагин реализует трейты для собственного типа. Реализовать `codec::Decode`
или `codec::Encode` непосредственно для `codec::Markdown` нельзя
по [правилам Rust](https://doc.rust-lang.org/reference/items/implementations.html#orphan-rules).

## API

### Преобразование

- `decode::<T>(data).await -> Result<T, T::Error>` — вызывает `T::decode` в пуле.
  Требуется `T: Decode + Send + 'static`, `T::Error: Send + 'static`.
  Аргумент `data` принимается как `impl AsRef<[u8]> + Send + 'static`.
- `encode(value).await -> Result<Vec<u8>, T::Error>` — вызывает `T::encode` в пуле
  и возвращает собственный буфер. Требуется `T: Encode + Send + 'static`,
  `T::Error: Send + 'static`.

Функции принимают данные по значению. Для декодирования подходят `String`, `Vec<u8>`
и строковые литералы. Для кодирования передавайте значение, например `Json(value)`.
Ссылки на локальные значения не удовлетворяют `'static`.

### Синхронные трейты

`Decode: Sized` задаёт разбор данных:

- `type Error` — тип ошибки декодирования.
- `decode(bytes: &[u8]) -> Result<Self, Self::Error>` — разбирает байты в значение.

`Encode` задаёт кодирование значения:

- `type Error` — тип ошибки кодирования.
- `encode(&self) -> Result<Cow<'_, [u8]>, Self::Error>` — возвращает представление в байтах.

Варианты результата `Encode`:

- `Cow::Borrowed` — ссылка на существующие байты.
- `Cow::Owned` — сформированный `Vec<u8>`.

Оба трейта синхронные, не требуют `Send` или `'static` и допускают работу
с заимствованными данными. Каждая реализация задаёт собственный тип ошибки.

### Форматы

Целый документ при разборе и результат одного вызова `Encode` должны помещаться в память.

#### Текст и байты

- `String` реализует `Decode`: проверяет UTF-8 и возвращает текст.
- `Vec<u8>` реализует `Decode`: копирует байты без преобразования.
- `str`, `String`, `[u8]`, `[u8; N]` и `Vec<u8>` реализуют `Encode`:
  текст возвращается как байты UTF-8, байты — без преобразования.
- `&T` реализует `Encode` при `T: Encode + ?Sized`, используя преобразование и ошибку `T`.

#### Json и Toml

- `Json<T>` содержит одно JSON-значение. После него во входных данных разрешены
  только пробельные символы. Кодирование даёт компактный JSON с завершающим `\n`.
- `Toml<T>` содержит документ UTF-8, разобранный по [правилам TOML](https://toml.io/en/).
  При кодировании комментарии и исходное оформление не сохраняются.

Значения создаются через `Json(value)` и `Toml(value)`. Оба типа предоставляют методы:

- `into_inner() -> T` — извлекает содержимое.
- `as_ref() -> &T` — возвращает ссылку через `AsRef<T>`.

Реализации трейтов независимы:

- `Decode` требует `T: serde::de::DeserializeOwned`.
- `Encode` требует `T: serde::Serialize`.

Синхронное кодирование поддерживает ссылки, например `Json(&value)`.
Методы `into_inner` и `as_ref` также синхронные.

#### Markdown

`Markdown` реализует `Decode` и `Encode` для [CommonMark](https://spec.commonmark.org/)
без расширений. Синхронные методы:

- `parse(text: &str) -> Markdown` — сохраняет копию текста UTF-8 без разбора структуры.
- `events() -> impl Iterator<Item = markdown::Event<'_>> + '_` — начинает новый
  синхронный обход элементов документа без кеширования событий.

`codec::markdown` реэкспортирует `Event`, `Tag`, `TagEnd` и связанные типы
[pulldown-cmark](https://docs.rs/pulldown-cmark/latest/pulldown_cmark/).

### Потоковые преобразования

`StreamDecode` и `StreamEncode` работают синхронно с данными в памяти.
Их можно реализовать независимо для собственного формата или протокола.
Трейты не требуют `Send` или `'static`. Асинхронный адаптер управляет вводом-выводом
и передаёт длительные преобразования в вычислительный пул.

#### StreamDecode

- `type Item` — тип одной разобранной записи.
- `type Error` — тип ошибки разбора.
- `push(&mut self, bytes: &[u8]) -> Result<(), Self::Error>` — принимает порцию байтов.
  Граница порции может проходить внутри значения или символа UTF-8.
- `next(&mut self) -> Result<Option<Self::Item>, Self::Error>` — возвращает готовое значение.
  До конца ввода `None` означает нехватку данных, после — конец последовательности.
- `finish(&mut self) -> Result<(), Self::Error>` — сообщает о конце ввода.
  После успешного завершения повторный вызов допустим, `push` — ошибка.

После каждого `push` и после `finish` вызывайте `next` до `None`.
Любая ошибка прекращает разбор.

#### StreamEncode

- `type Item: ?Sized` — тип кодируемого значения.
- `type Error` — тип ошибки кодирования.
- `encode(&mut self, item: &Self::Item, output: &mut Vec<u8>) -> Result<(), Self::Error>` —
  добавляет байты очередного значения в буфер.
- `finish(&mut self, output: &mut Vec<u8>) -> Result<(), Self::Error>` —
  добавляет завершение формата, если оно требуется.

Правила работы с буфером и завершением:

- Буфер принадлежит вызывающему коду. После передачи байтов потребителю его можно очистить.
- Успешный вызов добавляет байты, сохраняя существующее содержимое буфера.
- Ошибка оставляет буфер неизменным и запрещает дальнейшее кодирование.
- Повторный успешный `finish` ничего не добавляет. `encode` после `finish` — ошибка.
- Завершение кодера, отправка его байтов и закрытие потока ввода-вывода — отдельные операции.

#### JSONL

`Jsonl<T>` реализует потоковые трейты с `Item = T` и `Error = codec::Error`:

- `StreamDecode` — при `T: serde::de::DeserializeOwned`.
- `StreamEncode` — при `T: serde::Serialize`.

Ограничения и состояния чтения и записи независимы.
Конструктор `Jsonl::<T>::new()` и `Default` не требуют ни одного из ограничений.

Собственные методы `Jsonl<T>`:

- `new() -> Jsonl<T>` — создаёт кодек без входных и выходных записей.
- `push(&mut self, chunk: &[u8]) -> Result<(), Error>` — добавляет порцию байтов.
- `next(&mut self) -> Result<Option<T>, Error>` — разбирает одну доступную строку.
- `finish(&mut self)` — отмечает конец ввода. Повторный вызов допустим.

Методы `push`, `next` и `finish` требуют `T: serde::de::DeserializeOwned`.

Разбор следует правилам `StreamDecode`:

- Вход — UTF-8 без BOM, по одному JSON-значению на строку.
- Допустимы `\n`, `\r\n` и последняя строка без перевода.
- Пустая строка — ошибка. Пустой источник содержит ноль записей.
- Неполная строка, включая незавершённый символ UTF-8, сохраняется между вызовами `push`.
- Обработанные записи не накапливаются. В память должны помещаться непрочитанные байты
  и одна разобранная запись.
- После ошибки `next` возвращает `Ok(None)`.
- `push` после ошибки или `finish` возвращает `Error::Decode` с форматом `"jsonl"`.

Кодирование следует правилам `StreamEncode`:

- Каждая запись кодируется как `Json(value)` с завершающим `\n`.
- `StreamEncode::finish` не добавляет байтов.
- Ноль записей даёт пустой вывод.
- Ошибки кодирования имеют формат `"jsonl"`.

Вызовы завершения ввода и вывода различаются:

- `Jsonl::finish(&mut codec)` — завершает ввод и возвращает `()`.
- `StreamDecode::finish(&mut codec)` — завершает ввод и возвращает `Result<(), Error>`.
- `StreamEncode::finish(&mut codec, &mut output)` — завершает вывод и возвращает `Result<(), Error>`.

#### Decoder

`Decoder<T>` реализует `StreamDecode` при `T: Decode`.
`Item = T`, `Error = StreamError<T::Error>`.

- `Decoder::<T>::new()` — создаёт декодер одного документа. Также реализован `Default`.

Декодер накапливает байты и передаёт их целиком в `T::decode` после конца ввода:

- До `finish` метод `next` возвращает `None`.
- После `finish` первый `next` возвращает документ или ошибку преобразования.
  Последующие вызовы возвращают `Ok(None)`.
- Пустой ввод проверяется по правилам выбранного `Decode`.
- После ошибки `next` возвращает `Ok(None)`.

#### Encoder

`Encoder<T>` реализует `StreamEncode` при `T: Encode + ?Sized`.
`Item = T`, `Error = StreamError<T::Error>`.

- `Encoder::<T>::new()` — создаёт кодер одного документа. Также реализован `Default`.

Кодер использует существующий `Encode` и требует ровно одно значение:

- Второй вызов `encode` — ошибка.
- `finish` без переданного значения — ошибка. Для пустого текста или байтов передайте пустое значение.

### Выполнение

Способ выполнения задаётся вызовом и не зависит от размера данных:

- `codec::decode` и `codec::encode` передают преобразование в общий пул [fairway-compute](compute.md).
- Методы `Decode`, `Encode`, `StreamDecode` и `StreamEncode` выполняются на текущем потоке
  и не должны выполнять ввод-вывод.

Пул общий для процесса. Число потоков равно `std::thread::available_parallelism()`,
при невозможности его определить используется один поток. Настройка потоков Tokio
этот лимит не меняет. Ожидание свободного места асинхронное.

Преобразования, переданные в пул, не занимают рабочие потоки Tokio
и его пул блокирующего ввода-вывода.
Это соответствует [рекомендациям Tokio](https://docs.rs/tokio/latest/tokio/index.html#cpu-bound-tasks-and-blocking-code).

Ожидание свободного места можно отменить. Уже переданное в пул преобразование
выполняется, даже если оно ещё не успело начаться, и удерживает место до завершения.
Если ожидание отменено, результат отбрасывается.

### Ошибки

Тип ошибки зависит от преобразования:

- Декодирование `String` и `Markdown` — `std::str::Utf8Error` при некорректном UTF-8.
- Декодирование `Vec<u8>`, кодирование текста и байтов — `std::convert::Infallible`.
- JSON, TOML, JSONL и кодирование Markdown — `codec::Error`.
- Кодирование `&T` — ошибка `T::Error`.
- `Decoder<T>` и `Encoder<T>` — `StreamError<E>`, где `E` — ошибка выбранного преобразования.

#### Error

`codec::Error` реализует `std::error::Error`, `Send` и `Sync`. Варианты:

- `Decode { format, line, column, source }` — ошибка формата, состояния парсера или преобразования в тип.
- `Encode { format, source }` — ошибка преобразования значения в байты.

Поля ошибки:

- `format: &'static str` — имя формата: `"json"`, `"jsonl"`, `"toml"` или `"markdown"`.
- `line: Option<u64>` и `column: Option<u64>` — позиция, если она известна.
  Нумерация начинается с `1`. Для JSONL номер строки относится ко всему источнику.
- `source: Box<dyn std::error::Error + Send + Sync>` — исходная ошибка.

#### StreamError

- `Codec(E)` — исходная ошибка `Decode` или `Encode`.
- `Closed` — вход завершён или кодер уже принял документ.
- `Failed` — предыдущая ошибка прекратила преобразование.
- `MissingValue` — `Encoder::finish` вызван без документа.

`StreamError<E>` реализует `std::error::Error` при `E: std::error::Error + 'static`.
`source()` у `Codec(E)` возвращает исходную ошибку.
