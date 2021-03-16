-- Need to support old syntax without `drop column`
begin transaction;

create table update_log_old (time integer primary key, status integer, message text);
insert into update_log_old (time, status, message) 
    select time, status, message from update_log;

drop table update_log;
alter table update_log_old rename to update_log;

commit;
