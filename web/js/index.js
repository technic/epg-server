import $ from 'jquery'
import 'bootstrap'
import 'regenerator-runtime/runtime.js';

import bsCustomFileInput from 'bs-custom-file-input'
import fromUnixTime from 'date-fns/fromUnixTime'
import format from 'date-fns/format'

$(document).ready(function () {
    bsCustomFileInput.init()

    $("span.date").each(function () {
        const t = $(this).data('timestamp');
        $(this).text(format(fromUnixTime(t), "HH:mm"));
    });
});

import './table.js';
import './playlist.js';
import './../index.scss';
