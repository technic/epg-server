import $ from 'jquery';
import bsCustomFileInput from 'bs-custom-file-input';
let captcha;

window.onloadCallback = function() {
  class Recaptcha {
    constructor(elementId) {
      this.working = false;
      this.resolve = undefined;
      this.reject = undefined;
      this.widget_id = grecaptcha.render(elementId, {
        'callback': (token) => {
          this.working = false;
          this.resolve(token);
        },
        'error-callback': (error) => {
          this.working = false;
          this.reject(error);
        },
      }, true);
    }
    execute() {
      if (this.working) {
        throw 'Already working';
      }
      this.working = true;
      return new Promise((resolve, reject) => {
        this.resolve = resolve;
        this.reject = reject;
        grecaptcha.execute(this.widget_id);
      });
    }
    reset() {
      if (this.working) {
        throw 'Already working';
      }
      grecaptcha.reset();
    }
  }

  captcha = new Recaptcha('captcha');
};

$(function() {
  bsCustomFileInput.init();
});

$(function() {
  const changes = {};
  let currentItem;

  $('#uploadForm').on('submit', async function(ev) {
    ev.preventDefault();
    $('#resultRow').hide();
    $('#loader').show();
    if (this.checkValidity() === false) {
      ev.stopPropagation();
      return;
    }
    try {
      await captcha.execute();
      const f = document.getElementById('uploadForm');
      const formData = new FormData(f);
      const reply = await $.ajax({
        type: 'POST',
        url: './index.html',
        data: formData,
        processData: false,
        contentType: false,
      });
      $('#loader').hide();
      $('#resultRow').show();
      $('#tableContainer').html(reply);
      $('#tableContainer').find('.btn.btn-primary').on('click', edit);
      $('#tableContainer').find('.btn.btn-secondary').on('click', markOk);
    } catch (error) {
      alert(error);
    } finally {
      captcha.reset();
    }
  });

  function itemName($tr_item) {
    return $tr_item.children('td').eq(1).text();
  }

  async function edit() {
    currentItem = $(this).closest('tr');
    const name = itemName(currentItem);
    const result = await $.ajax({
      url: './find',
      type: 'POST',
      dataType: 'json',
      data: {name: name},
    });
    const list = $('#suggestions');
    list.empty();
    for (const item of result['data']) {
      list.append($('<li>').addClass('list-group-item').addClass('list-group-item-action').text(item['name'])
          .append(`<a class="btn btn-sm" href="../programs.html?id=${item['alias']}" target="_blank"><i class="fas fa-info-circle"></i></a>`));
    }
    $('#setButton').attr('disabled', true);
    const modal = $('#listDialog');
    modal.find('.modal-title').text(`Suggestions for "${name}"`);
    modal.modal('show');
  };

  function markOk() {
    $(this).closest('tr').removeClass('table-warning');
  }

  $('#suggestions').on('click', 'li', function() {
    $(this).addClass('active').siblings().removeClass('active');
    $('#setButton').attr('disabled', false);
  });

  $('#suggestions').on('click', 'a', function(e) {
    e.stopPropagation();
  });

  $('#setButton').on('click', function() {
    const text = $('#suggestions').find('li.active').text();
    setText(text);
  });

  $('#clearButton').on('click', function() {
    const text = '';
    setText(text);
  });

  function setText(text) {
    $('#listDialog').modal('hide');
    changes[itemName(currentItem)] = text;
    currentItem.children('td').eq(2).text(text);
    currentItem.removeClass('table-danger').removeClass('table-warning');
  }

  $('#downloadButton').on('click', async function() {
    try {
      const token = await captcha.execute();
      const formData = new FormData(document.getElementById('downloadForm'));
      formData.set('changes', JSON.stringify(changes));
      formData.set('g-recaptcha-response', token);
      const playlist = await $.ajax({
        type: 'POST',
        url: './get_m3u',
        data: formData,
        processData: false,
        contentType: false,
      });
      const blob = new Blob([playlist], {type: 'application/mpegurl'});
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url; a.download = 'playlist.m3u'; a.click();
    } catch (error) {
      alert(error);
    } finally {
      captcha.reset();
    }
  });
});
