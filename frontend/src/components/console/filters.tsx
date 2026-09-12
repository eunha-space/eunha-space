import { Search } from "lucide-react";
import { Input } from "@/components/ui/input";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";

export function SearchField({
  value,
  onChange,
  placeholder,
  label,
}: {
  value: string;
  onChange: (value: string) => void;
  placeholder: string;
  label: string;
}) {
  return (
    <div className="relative">
      <Search className="text-muted-foreground pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2" />
      <Input
        type="search"
        value={value}
        aria-label={label}
        placeholder={placeholder}
        onChange={(event) => onChange(event.target.value)}
        className="w-full pl-8 sm:w-64"
      />
    </div>
  );
}

export type Choice = { value: string; label: string; count?: number };

export function ChipFilter({
  choices,
  value,
  onChange,
  label,
}: {
  choices: Choice[];
  value: string;
  onChange: (value: string) => void;
  label: string;
}) {
  return (
    <ToggleGroup
      type="single"
      variant="outline"
      size="sm"
      value={value}
      aria-label={label}
      // Radix clears the value when the active item is pressed again; keep the current filter.
      onValueChange={(next) => onChange(next || value)}
      className="flex-wrap"
    >
      {choices.map((choice) => (
        <ToggleGroupItem key={choice.value} value={choice.value} className="gap-1.5 rounded-full px-3">
          {choice.label}
          {choice.count !== undefined && (
            <span className="text-muted-foreground text-xs tabular-nums">{choice.count}</span>
          )}
        </ToggleGroupItem>
      ))}
    </ToggleGroup>
  );
}
