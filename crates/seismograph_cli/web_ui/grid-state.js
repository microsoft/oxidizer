// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });

export function gridRows(rows, columns, query, sort) {
  const search = query.trim().toLocaleLowerCase();
  const visible = rows.filter(row => !search || columns.some(({ value }) =>
    String(value(row) ?? "").toLocaleLowerCase().includes(search)));
  const column = columns.find(({ name }) => name === sort?.column);
  if (!column) return visible;
  const value = column.sortValue ?? column.value;
  return visible.sort((a, b) => {
    const left = value(a);
    const right = value(b);
    if (left == null || right == null) return Number(left == null) - Number(right == null);
    const integers = /^-?\d+$/;
    let order;
    if (integers.test(String(left)) && integers.test(String(right))) {
      const comparison = BigInt(left) - BigInt(right);
      order = comparison < 0n ? -1 : comparison > 0n ? 1 : 0;
    } else if (typeof left === "number" && typeof right === "number") {
      order = left - right;
    } else {
      order = collator.compare(String(left), String(right));
    }
    return sort.descending ? -order : order;
  });
}
