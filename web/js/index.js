import 'bootstrap/dist/js/bootstrap.js'
import 'regenerator-runtime/runtime.js';

import './../style.css';

import $ from 'jquery'
import bsCustomFileInput from 'bs-custom-file-input'
import moment from 'moment'

$(document).ready(function () {
    bsCustomFileInput.init()

    $("span.date").each(function () {
        const t = $(this).data('timestamp');
        $(this).text(moment.unix(t).format("HH:mm"));
    });
});

import './table.js';
import './playlist.js';
