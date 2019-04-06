create table channels_new
    (id integer primary key, alias text unique, name text, icon_url text);
insert into channels_new (id, alias, name, icon_url) select id, cast(id as text), name, icon_url from channels;
drop table channels;
alter table channels_new rename to channels;
