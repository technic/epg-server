import $ from 'jquery';
var captcha;

window.onloadCallback = function () {
    class Recaptcha {
        constructor(elementId) {
            this.working = false;
            this.resolve = undefined;
            this.reject = undefined;
            this.widget_id = grecaptcha.render(elementId, {
                "callback": (token) => {
                    this.working = false;
                    this.resolve(token)
                },
                "error-callback": (error) => {
                    this.working = false;
                    this.reject(error)
                }
            }, true);
        }
        execute() {
            if (this.working) {
                throw "Already working"
            }
            this.working = true;
            return new Promise((resolve, reject) => {
                this.resolve = resolve;
                this.reject = reject;
                grecaptcha.execute(this.widget_id);
            })
        }
        reset() {
            if (this.working) {
                throw "Already working"
            }
            grecaptcha.reset();
        }
    }

    captcha = new Recaptcha('captcha');
}

$(document).ready(function () {
    var changes = {};
    var current_item;

    $('#uploadForm').submit(async function (ev) {
        ev.preventDefault();
        $('#resultRow').hide();
        $('#loader').show();
        if (this.checkValidity() === false) {
            ev.stopPropagation();
            return
        }
        try {
            await captcha.execute();
            var f = document.getElementById('uploadForm');
            var formData = new FormData(f);
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
            $('#tableContainer').find('.btn.btn-primary').click(edit);
            $('#tableContainer').find('.btn.btn-secondary').click(markOk);
        } catch (error) {
            alert(error);
        } finally {
            captcha.reset();
        }
    })

    function itemName($tr_item) {
        return $tr_item.children("td").eq(1).text();
    }

    async function edit() {
        current_item = $(this).closest("tr");
        const name = itemName(current_item);
        const result = await $.ajax({
            url: './find',
            type: 'POST',
            dataType: 'json',
            data: { name: name },
        });
        var list = $('#suggestions');
        list.empty();
        for (var item of result['data']) {
            list.append($("<li>").addClass("list-group-item").addClass("list-group-item-action").text(item['name'])
                .append(`<a class="btn btn-sm" href="../programs.html?id=${item['alias']}" target="_blank"><i class="fas fa-info-circle"></i></a>`));
        }
        $('#setButton').attr('disabled', true);
        var modal = $('#listDialog');
        modal.find('.modal-title').text(`Suggestions for "${name}"`);
        modal.modal('show');
    };

    function markOk() {
        $(this).closest("tr").removeClass("table-warning");
    }

    $("#suggestions").on("click", "li", function (e) {
        $(this).addClass("active").siblings().removeClass("active");
        $('#setButton').attr('disabled', false);
    });

    $("#suggestions").on("click", "a", function (e) {
        e.stopPropagation();
    })

    $('#setButton').click(function () {
        const text = $("#suggestions").find('li.active').text();
        setText(text);
    })

    $('#clearButton').click(function () {
        const text = "";
        setText(text);
    })

    function setText(text) {
        $('#listDialog').modal('hide');
        changes[itemName(current_item)] = text;
        current_item.children("td").eq(2).text(text);
        current_item.removeClass('table-danger').removeClass('table-warning');
    }

    $("#downloadButton").click(async function () {
        try {
            const token = await captcha.execute();
            var formData = new FormData(document.getElementById('downloadForm'));
            formData.set('changes', JSON.stringify(changes));
            formData.set('g-recaptcha-response', token);
            const playlist = await $.ajax({
                type: 'POST',
                url: './get_m3u',
                data: formData,
                processData: false,
                contentType: false,
            });
            var blob = new Blob([playlist], { type: 'application/mpegurl' });
            var url = URL.createObjectURL(blob);
            var a = document.createElement('a');
            a.href = url; a.download = "playlist.m3u"; a.click();
        } catch (error) {
            alert(error);
        } finally {
            captcha.reset();
        }
    })
});
