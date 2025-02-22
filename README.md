# LIT (Language Immersion Tool)

This is a very rough language-learning reader application, based on
[Lute](https://github.com/LuteOrg/lute-v3), that supports some key features I
found useful for studying Korean in particular, but which would have been
difficult to integrate into an upstream contribution.

This is currently unpolished and should be considered "early alpha". If you're
looking for something more stable, please consider using
[Lute](https://github.com/LuteOrg/lute-v3).

## Setup

- Create a directory for your LIT database. I use "./data/prod" for my
  production instance and "data/dev" for a development instance. Note that
  "./data" is, conveniently, included in .gitignore.
- Create an empty database using the provided schema:
  ```
  ENV=prod
  sqlite data/${ENV?}/lit.db < schema/db.sql
  ```
- Copy the example config (`example/config/config.toml`) into the same directory
  as your database.
- Download or build the Korean Mecab dictionary files (see instructions in the
  `[mecab]` section of `config.toml`).
- Import some books. There is a CLI tool, `src/bin/add_book.rs`. See
  `example/scripts/import_youtube.py` for example usage, or run the following
  to see a list of all options:
  ```
  cargo run --release --bin add_book -- --help
  ```
  Alternatively, you can import Youtube videos that have Korean subtitles by
  navigating to `http://localhost:5080/video?url=<YOUTUBE_WATCH_URL>`.
- Run the reader:
  ```
  ENV=prod
  cargo run --release --bin lit -- --config=data/${ENV?}/config.toml
  ```
- Open the reader in a web browser (note that there isn't any top-level
  navigation implemented yet):
  - List of books: http://localhost:5080/books
  - List of words: http://localhost:5080/words

## License

Copyright (c) 2025 Brad Kimmel

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
