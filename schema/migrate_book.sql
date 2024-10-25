begin;

drop trigger if exists book_ai;
drop trigger if exists book_ad;
drop trigger if exists book_au;
drop trigger if exists book_tag_ai;
drop trigger if exists book_tag_ad;
drop trigger if exists book_tag_au;
drop table if exists book_fts;

drop index if exists book_added;
drop index if exists book_published;
drop index if exists book_last_read;

alter table book rename to book_old;
alter table book_tag rename to book_tag_old;

create table book (
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

create table book_tag (
  book_id integer not null references book(id),
  tag     varchar not null check(tag <> '')
);

insert into book (id, title, url, added, published, last_read, archived, audio_file, content_type, content)
select id, title, url, added, published, last_read, archived, audio_file, content_type, content
from book_old;

insert into book_tag (book_id, tag)
select book_id, tag
from book_tag_old;

drop table book_tag_old;
drop table book_old;

create index book_added on book (added);
create index book_published on book (published);
create index book_last_read on book (last_read);

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
