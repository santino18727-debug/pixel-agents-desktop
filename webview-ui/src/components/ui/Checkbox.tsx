// Based on pixel-agents by pablodelucca (https://github.com/pablodelucca/pixel-agents)
// Licensed under MIT
interface CheckboxProps {
  checked: boolean;
  onChange: () => void;
  label: string;
  /** Optional one-line description shown under the label. */
  description?: string;
  className?: string;
}

export function Checkbox({ checked, onChange, label, description, className = '' }: CheckboxProps) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      onClick={onChange}
      className={`flex items-center justify-between w-full py-6 px-10 gap-8 bg-transparent border-none rounded-none cursor-pointer text-left hover:bg-btn-bg ${className}`}
    >
      <span className="flex flex-col">
        <span>{label}</span>
        {description && <span className="text-2xs text-text-muted leading-normal">{description}</span>}
      </span>
      <span
        aria-hidden="true"
        className={`w-14 h-14 border-2 border-white/50 rounded-none shrink-0 flex items-center justify-center text-sm leading-none text-white ${checked ? 'bg-accent' : 'bg-transparent'}`}
      >
        {checked ? '✓' : ''}
      </span>
    </button>
  );
}
