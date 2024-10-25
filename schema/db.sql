pragma foreign_keys = on;

create table if not exists book (
  id           integer  not null primary key,
  slug         varchar  unique check(slug <> '' and slug not like '%/%'),
  title        varchar  not null unique check(title <> ''),
  url          varchar  check(url <> ''),
  added        datetime not null default current_timestamp,
  published    datetime,
  last_read    datetime,
  archived     boolean  not null default false,
  audio_file   varchar  check(audio_file <> ''),
  content_type varchar  not null check(content_type <> ''),
  content      text     not null
);
create index if not exists book_added on book (added);
create index if not exists book_published on book (published);
create index if not exists book_last_read on book (last_read);

create table if not exists book_tag (
  book_id integer not null references book(id),
  tag     varchar not null check(tag <> '')
);

begin;

drop trigger if exists book_ai;
drop trigger if exists book_ad;
drop trigger if exists book_au;
drop trigger if exists book_tag_ai;
drop trigger if exists book_tag_ad;
drop trigger if exists book_tag_au;
drop table if exists book_fts;

create virtual table if not exists book_fts using fts5 (title, content, tags, content='', contentless_delete=1);

create trigger if not exists book_ai after insert on book begin
  insert into book_fts (rowid, title, content, tags)
  values (new.id, new.title, new.content, null);
end;
create trigger if not exists book_ad after delete on book begin
  delete from book_fts where rowid = old.id;
end;
create trigger if not exists book_au after update on book begin
  delete from book_fts where rowid = old.id;
  insert into book_fts (rowid, title, content, tags)
  select id, title, content, group_concat(tag,'|') as tags
  from book
    inner join book_tag on id = book_id
  where id = new.id
  group by id;
end;
create trigger if not exists book_tag_ai after insert on book_tag begin
  delete from book_fts where rowid = new.book_id;
  insert into book_fts (rowid, title, content, tags)
  select id, title, content, group_concat(tag,'|') as tags
  from book
    inner join book_tag on id = book_id
  where id = new.book_id
  group by id;
end;
create trigger if not exists book_tag_ad after delete on book_tag begin
  delete from book_fts where rowid = old.book_id;
  insert into book_fts (rowid, title, content, tags)
  select id, title, content, group_concat(tag,'|') as tags
  from book
    inner join book_tag on id = book_id
  where id = old.book_id
  group by id;
end;
create trigger if not exists book_tag_au after update on book_tag begin
  delete from book_fts where rowid in (old.book_id, new.book_id);
  insert into book_fts (rowid, title, content, tags)
  select id, title, content, group_concat(tag,'|') as tags
  from book
    inner join book_tag on id = book_id
  where id in (old.book_id, new.book_id)
  group by id;
end;

insert into book_fts (rowid, title, content, tags)
select id, title, content, group_concat(tag, '|') as tags
from book
  left outer join book_tag on id = book_id
group by id;

commit;

create table if not exists word (
  id             integer  not null primary key,
  text           varchar  not null check(text <> ''),
  pronunciation  varchar  check(pronunciation <> ''),
  translation    text     not null,
  status         tinyint  default 1 check(status <> 0),
  added          datetime not null default 'now',
  image_file     varchar  check(image_file <> '')
);
create index if not exists word_text on word (text);
create index if not exists word_added on word (added);

create table if not exists word_tag (
  word_id integer not null references word(id),
  tag     varchar not null check(tag <> ''),
  primary key (word_id, tag)
);

create table if not exists word_parent (
  child_word_id    integer not null references word(id),
  parent_word_text varchar not null check(parent_word_text <> ''),
  primary key (child_word_id, parent_word_text)
);

begin;

drop trigger if exists word_ai;
drop trigger if exists word_ad;
drop trigger if exists word_au;
drop trigger if exists word_parent_ai;
drop trigger if exists word_parent_ad;
drop trigger if exists word_parent_au;
drop trigger if exists word_tag_ai;
drop trigger if exists word_tag_ad;
drop trigger if exists word_tag_au;
drop table if exists word_fts;

create virtual table if not exists word_fts using fts5 (text, pronunciation, translation, parents, tags, content='', contentless_delete=1);

create trigger if not exists word_ai after insert on word begin
  insert into word_fts (rowid, text, pronunciation, translation)
  values (new.id, new.text, new.pronunciation, new.translation);
end;
create trigger if not exists word_ad after delete on word begin
  delete from word_fts where rowid = old.id;
end;
create trigger if not exists word_au after update on word begin
  delete from word_fts where rowid = old.id;
  insert into word_fts (rowid, text, pronunciation, translation, parents, tags)
  select
    id, text, pronunciation, translation,
    (select group_concat(parent_word_text,'|') from word_parent where child_word_id = id),
    (select group_concat(tag,'|') from word_tag where word_id = id)
  from word
  where id = new.id;
end;
create trigger if not exists word_parent_ai after insert on word_parent begin
  delete from word_fts where rowid = new.child_word_id;
  insert into word_fts (rowid, text, pronunciation, translation, parents, tags)
  select
    id, text, pronunciation, translation,
    (select group_concat(parent_word_text,'|') from word_parent where child_word_id = id),
    (select group_concat(tag,'|') from word_tag where word_id = id)
  from word
  where id = new.child_word_id;
end;
create trigger if not exists word_parent_ad after delete on word_parent begin
  delete from word_fts where rowid = old.child_word_id;
  insert into word_fts (rowid, text, pronunciation, translation, parents, tags)
  select
    id, text, pronunciation, translation,
    (select group_concat(parent_word_text,'|') from word_parent where child_word_id = id),
    (select group_concat(tag,'|') from word_tag where word_id = id)
  from word
  where id = old.child_word_id;
end;
create trigger if not exists word_parent_au after update on word_parent begin
  delete from word_fts where rowid in (old.child_word_id, new.child_word_id);
  insert into word_fts (rowid, text, pronunciation, translation, parents, tags)
  select
    id, text, pronunciation, translation,
    (select group_concat(parent_word_text,'|') from word_parent where child_word_id = id),
    (select group_concat(tag,'|') from word_tag where word_id = id)
  from word
  where id in (old.child_word_id, new.child_word_id);
end;
create trigger if not exists word_tag_ai after insert on word_tag begin
  delete from word_fts where rowid = new.word_id;
  insert into word_fts (rowid, text, pronunciation, translation, parents, tags)
  select
    id, text, pronunciation, translation,
    (select group_concat(parent_word_text,'|') from word_parent where child_word_id = id),
    (select group_concat(tag,'|') from word_tag where word_id = id)
  from word
  where id = new.word_id;
end;
create trigger if not exists word_tag_ad after delete on word_tag begin
  delete from word_fts where rowid = old.word_id;
  insert into word_fts (rowid, text, pronunciation, translation, parents, tags)
  select
    id, text, pronunciation, translation,
    (select group_concat(parent_word_text,'|') from word_parent where child_word_id = id),
    (select group_concat(tag,'|') from word_tag where word_id = id)
  from word
  where id = old.word_id;
end;
create trigger if not exists word_tag_au after update on word_tag begin
  delete from word_fts where rowid in (old.word_id, new.word_id);
  insert into word_fts (rowid, text, pronunciation, translation, parents, tags)
  select
    id, text, pronunciation, translation,
    (select group_concat(parent_word_text,'|') from word_parent where child_word_id = id),
    (select group_concat(tag,'|') from word_tag where word_id = id)
  from word
  where id in (old.word_id, new.word_id);
end;

insert into word_fts (rowid, text, pronunciation, translation, parents, tags)
select id, text, pronunciation, translation,
  (select group_concat(parent_word_text,'|') from word_parent where child_word_id = id) as parents,
  (select group_concat(tag,'|') from word_tag where word_id = id) as tags
from word;

commit;
