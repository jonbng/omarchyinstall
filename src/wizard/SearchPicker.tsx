import { useEffect, useId, useMemo, useRef, useState } from "react";
import type { PickerOption } from "./options";

export function SearchPicker({
  label,
  value,
  selectedLabel,
  options,
  onChange,
  onSearch,
}: {
  label: string;
  value: string;
  selectedLabel?: string;
  options: PickerOption[];
  onChange: (option: PickerOption) => void;
  onSearch?: () => void;
}) {
  const id = useId();
  const rootRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const selected = options.find(
    (option) => option.id === value && (!selectedLabel || option.label === selectedLabel),
  );
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState(selected?.label ?? value);
  const [active, setActive] = useState(0);

  const filtered = useMemo(() => {
    const needle = query.trim().toLocaleLowerCase();
    if (!needle || needle === selected?.label.toLocaleLowerCase()) return options;
    return options.filter((option) =>
      `${option.label} ${option.id}`.toLocaleLowerCase().includes(needle),
    );
  }, [options, query, selected?.label]);

  useEffect(() => {
    if (!open) setQuery(selected?.label ?? value);
  }, [open, selected?.label, value]);

  useEffect(() => {
    const close = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("pointerdown", close);
    return () => document.removeEventListener("pointerdown", close);
  }, []);

  useEffect(() => {
    if (!open) return;
    document.getElementById(`${id}-option-${active}`)?.scrollIntoView({ block: "nearest" });
  }, [active, id, open]);

  function choose(option: PickerOption) {
    onChange(option);
    setQuery(option.label);
    setOpen(false);
    inputRef.current?.focus();
  }

  return (
    <div
      className="field-card picker"
      ref={rootRef}
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setOpen(false);
      }}
    >
      <label htmlFor={`${id}-input`}>{label}</label>
      <div className="picker-control">
        <input
          ref={inputRef}
          id={`${id}-input`}
          role="combobox"
          aria-autocomplete="list"
          aria-expanded={open}
          aria-controls={`${id}-listbox`}
          aria-activedescendant={open && filtered[active] ? `${id}-option-${active}` : undefined}
          autoComplete="off"
          spellCheck={false}
          value={query}
          onFocus={(event) => {
            setOpen(true);
            setActive(Math.max(0, options.findIndex((option) => option.id === value)));
            event.currentTarget.select();
          }}
          onChange={(event) => {
            setQuery(event.target.value);
            onSearch?.();
            setActive(0);
            setOpen(true);
          }}
          onKeyDown={(event) => {
            if (event.key === "ArrowDown") {
              event.preventDefault();
              setOpen(true);
              setActive((current) => Math.min(current + 1, Math.max(filtered.length - 1, 0)));
            } else if (event.key === "ArrowUp") {
              event.preventDefault();
              setOpen(true);
              setActive((current) => Math.max(current - 1, 0));
            } else if (event.key === "Enter" && open) {
              event.preventDefault();
              const option = filtered[active];
              if (option) choose(option);
            } else if (event.key === "Escape" && open) {
              event.preventDefault();
              setOpen(false);
              setQuery(selected?.label ?? value);
            }
          }}
        />
        <button
          type="button"
          className="picker-toggle"
          aria-label={open ? `Close ${label}` : `Open ${label}`}
          onClick={() => {
            setOpen((current) => !current);
            inputRef.current?.focus();
          }}
        >
          {open ? "▴" : "▾"}
        </button>
      </div>
      {open && (
        <div className="picker-menu" id={`${id}-listbox`} role="listbox">
          {filtered.length ? filtered.map((option, index) => (
            <button
              type="button"
              tabIndex={-1}
              role="option"
              aria-selected={option.id === value}
              id={`${id}-option-${index}`}
              key={`${option.id}-${option.label}`}
              className={`${index === active ? "active" : ""} ${option.id === value ? "selected" : ""}`}
              onPointerMove={() => setActive(index)}
              onMouseDown={(event) => event.preventDefault()}
              onClick={() => choose(option)}
            >
              <span>{option.label}</span>
              <small>{option.id}</small>
            </button>
          )) : (
            <p className="picker-empty">No matches</p>
          )}
        </div>
      )}
    </div>
  );
}
