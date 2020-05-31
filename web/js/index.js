import 'bootstrap/dist/js/bootstrap.js'
import 'regenerator-runtime/runtime.js';

import './../style.scss';

import $ from 'jquery'
import bsCustomFileInput from 'bs-custom-file-input'
import fromUnixTime from 'date-fns/fromUnixTime'

$(document).ready(function () {
    bsCustomFileInput.init()

    $("span.date").each(function () {
        const t = $(this).data('timestamp');
        $(this).text(fromUnixTime(t).format("HH:mm"));
    });
});

import './table.js';
import './playlist.js';
