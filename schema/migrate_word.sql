begin;

drop index if exists word_added;

alter table word rename to word_old;
alter table word_tag rename to word_tag_old;
alter table word_parent rename to word_parent_old;

create table word (
  id             integer  not null primary key,
  text           varchar  not null check(text <> ''),
  pronunciation  varchar  check(pronunciation <> ''),
  translation    text     not null,
  status         tinyint  default 1 check(status <> 0),
  added          datetime not null default current_timestamp,
  image_file     varchar  check(image_file <> '')
);

create table word_tag (
  word_id integer not null references word(id),
  tag     varchar not null check(tag <> ''),
  primary key (word_id, tag)
);

create table word_parent (
  child_word_id    integer not null references word(id),
  parent_word_text varchar not null check(parent_word_text <> ''),
  primary key (child_word_id, parent_word_text)
);

insert into word (id, text, pronunciation, translation, status, added, image_file)
select id, text, pronunciation, translation, status, added, image_file
from word_old;

insert into word_tag (word_id, tag)
select word_id, tag
from word_tag_old;

insert into word_parent (child_word_id, parent_word_text)
select child_word_id, parent_word_text
from word_parent_old;

drop table word_old;
drop table word_tag_old;
drop table word_parent_old;

create index word_added on word (added);

commit;
