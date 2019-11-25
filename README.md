[![CircleCI](https://circleci.com/bb/iptvdream/epg-server.svg?style=svg)](https://circleci.com/bb/iptvdream/epg-server)

This program serves XMLTV epg in json format. Epg can be returned by day for the specific channel and by time slice for all channels.
Program is written in [rust](https://www.rust-lang.org/) using [iron](http://ironframework.io/) web framework.

## Customization of systemd unit
After you placed a template file to `/etc/systemd/system` use 
```
sudo systemctl edit epg-server@foobar
```
And customize as
```systemd
[Service]
Environment=APP_URL=http://foo.bar/xmltv.xml
Environment=APP_PORT=3000
Environment=APP_DB=/path/to/epg.db
```

## The example nginx configuration
To forward requests to epg-server service you may use configuration like this
```
location /epg-foobar/ {
    proxy_set_header X-Real-IP $remote_addr;
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    proxy_set_header Host $http_host;
    proxy_set_header X-NginX-Proxy true;
    proxy_pass http://localhost:3000/;
    proxy_redirect off;
}
``` 

## Note to developer
- Keep code clean by using `cargo clippy`
- Keep dependencies updated with `cargo update`
