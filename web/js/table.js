import $ from 'jquery';
import fromUnixTime from 'date-fns/fromUnixTime';
import format from 'date-fns/format';

$(function() {
  $('span.date').each(function() {
    const t = $(this).data('timestamp');
    $(this).text(format(fromUnixTime(t), 'HH:mm'));
  });
});

$(function() {
  $('.filterable .btn-filter').on('click', function() {
    const $panel = $(this).parents('.filterable');
    const $filters = $panel.find('.filters input');
    const $tbody = $panel.find('.table tbody');
    if ($filters.prop('disabled') == true) {
      $filters.prop('disabled', false);
      $filters.first().trigger("focus");
    } else {
      $filters.val('').prop('disabled', true);
      $tbody.find('.no-result').remove();
      $tbody.find('tr').show();
    }
  });

  $('.filterable .filters input').on('keyup', function(e) {
    /* Ignore tab key */
    if (e.key == 'Tab') return;
    /* Useful DOM data and selectors */
    const $input = $(this);
    const inputContent = $input.val().toLowerCase();
    const $panel = $input.parents('.filterable');
    const column = $panel.find('.filters th').index($input.parents('th'));
    const $table = $panel.find('.table');
    const $rows = $table.find('tbody tr');
    /* Dirtiest filter function ever ;) */
    const $filteredRows = $rows.filter(function() {
      const value = $(this).find('td').eq(column).text().toLowerCase();
      return value.indexOf(inputContent) === -1;
    });
    /* Clean previous no-result if exist */
    $table.find('tbody .no-result').remove();
    /* Show all rows, hide filtered ones (never do that outside of a demo ! xD) */
    $rows.show();
    $filteredRows.hide();
    /* Prepend no-result row if all rows are filtered */
    if ($filteredRows.length === $rows.length) {
      $table.find('tbody').prepend($('<tr class="no-result text-center"><td colspan="' + $table.find('.filters th').length + '">No result found</td></tr>'));
    }
  });
});
