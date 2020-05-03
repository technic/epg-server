alter database character set = 'utf8';

create table channels (
  `id` bigint auto_increment primary key not null,
  `alias` varchar(1024) unique,
  `name` varchar(1024),
  `icon_url` varchar(4096)
);
create table programs (
  `id` int auto_increment primary key not null,
  `channel` bigint not null,
  `begin` bigint unsigned not null,
  `end` bigint unsigned not null,
  `title` varchar(16384),
  `description` text,
  foreign key (`channel`) references `channels`(`id`)
);
create table programs1 (
  `id` int auto_increment primary key not null,
  `channel` bigint not null,
  `begin` bigint unsigned not null,
  `end` bigint unsigned not null,
  `title` varchar(16384),
  `description` text,
  foreign key (`channel`) references `channels`(`id`)
);

create index channel on programs (channel)
