create table channels_old
    (id integer primary key, name text, icon_url text);
insert into channels_old (id, name, icon_url) select id, name, icon_url from channels;
drop table channels;
alter table channels_old rename to channels;
