from __future__ import print_function
from datetime import datetime
import requests


def check_epg(url):
    ts = int((datetime.now() - datetime(1970, 1, 1)).total_seconds())

    r = requests.get("%s/channels_names" % url)
    data = r.json()['data']
    total_channels = len(data)

    print('Getting epg from', url, "at", ts)
    r = requests.get('%s/epg_list?time=%d' % (url, ts))
    data = r.json()

    def f(channel):
        ps = channel['programs']
        if ps:
            p = ps[0]
            tb, te = p['begin'], p['end']
            # print([str(datetime.fromtimestamp(x)) for x in (tb, ts, te)])
            return tb < ts <= te
        else:
            return None

    epg_list = list(map(f, data['data']))
    coverage = float(epg_list.count(True)) / len(epg_list) * 100
    print("Epg present on %.2f %% of %d channels from %d total" % (coverage, len(epg_list), total_channels))
    # print(egpList)


if __name__ == "__main__":
    def _check_epg(url):
        try:
            check_epg(url)
        except Exception as ex:
            print(ex)
    _check_epg('http://iptvdream.zapto.org/epg-soveni')
    _check_epg('http://iptvdream.zapto.org/epg-iptvxone')
    _check_epg('http://iptvdream.zapto.org/epg-1ott')
    _check_epg('http://iptvdream.zapto.org/epg-shara-tv')
    _check_epg('http://iptvdream.zapto.org/epg-sharovoz')
    _check_epg('http://iptvdream.zapto.org/epg-itvlive')
    _check_epg('http://iptvdream.zapto.org/epg-fox')
