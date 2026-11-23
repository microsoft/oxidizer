// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

export function CloseButton({ label, onClick, disabled }) {
  return <button type="button" className="close-icon" aria-label={label} title={label}
    onClick={onClick} disabled={disabled}><span aria-hidden="true">×</span></button>;
}

export function DetailCard({ title, fields }) {
  if (!title && !fields.length) return null;
  const named = fields.filter(field => field.name);
  const notes = fields.filter(field => !field.name);
  return <div className="detail-card">
    {title && <h3>{title}</h3>}
    {!!named.length && <dl className="detail-fields">{named.map(({ name, value }, index) => <div key={index}>
      <dt>{name}</dt><dd>{value}</dd>
    </div>)}</dl>}
    {!!notes.length && <ul className="detail-notes">{notes.map(({ value }, index) => <li key={index}>{value}</li>)}</ul>}
  </div>;
}

export function ViewHeading({ kind, title, filename, context, children }) {
  return <div className="page-heading">
    <div className="heading-copy">
      <div className="heading-title">
        {kind && <span className="view-kind">{kind}</span>}
        <h1 className="view-title" tabIndex={-1} title={filename ?? title}>{title}</h1>
      </div>
      {context && <p className="view-context">{context}</p>}
    </div>
    {children}
  </div>;
}

export function ViewTabs({ id, label, items, selected, onSelect, inspector }) {
  function moveTab(event, index) {
    const count = items.length;
    const offsets = { ArrowRight: (index + 1) % count, ArrowLeft: (index + count - 1) % count, Home: 0, End: count - 1 };
    if (!Object.hasOwn(offsets, event.key)) return;
    const next = offsets[event.key];
    event.preventDefault();
    onSelect(items[next].id);
    event.currentTarget.parentElement.children[next].focus();
  }

  return <div className={inspector ? "section-tabs inspector-tabs" : "section-tabs"} role="tablist" aria-label={label}>{items.map((item, index) =>
    <button key={item.id} type="button" role="tab" id={`${id}-tab-${item.id}`}
      aria-controls={`${id}-panel-${item.id}`} aria-selected={selected === item.id} tabIndex={selected === item.id ? 0 : -1}
      onClick={() => onSelect(item.id)} onKeyDown={event => moveTab(event, index)}>{item.title}</button>
  )}</div>;
}

const elapsed = new Intl.NumberFormat(undefined, { maximumSignificantDigits: 3 });

export function TimeRuler({ seconds }) {
  return <span className="time-ruler">
    <span>0 s</span><span>{elapsed.format(seconds)} s</span>
  </span>;
}

export function ViewPanel({ id, item, selected, children }) {
  return <div role="tabpanel" id={`${id}-panel-${item}`} aria-labelledby={`${id}-tab-${item}`}
    hidden={selected !== item} tabIndex={0}>{children}</div>;
}
