import { useEffect, useState } from "react";
import { Moon, Sun } from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "cn";

export function Mark({ className }: { className?: string }) {
  return (
    <span className={cn("grid size-13 place-items-center", className)}>
      <img className="brand-symbol-light col-start-1 row-start-1 size-full object-contain" src="/eunha-symbol-light.svg" alt="" />
      <img className="brand-symbol-dark col-start-1 row-start-1 size-full object-contain" src="/eunha-symbol-dark.svg" alt="" />
    </span>
  );
}

/**
 * The initial value is written to the document by an inline script in
 * index.html, so the first paint already carries the right theme.
 */
export function ThemeToggle() {
  const [dark, setDark] = useState(() => document.documentElement.classList.contains("dark"));

  useEffect(() => {
    document.documentElement.classList.toggle("dark", dark);
    localStorage.theme = dark ? "dark" : "light";
  }, [dark]);

  const label = dark ? "Switch to the light theme" : "Switch to the dark theme";
  return (
    <Button variant="outline" size="icon" onClick={() => setDark(!dark)} aria-label={label} title={label}>
      {dark ? <Sun /> : <Moon />}
    </Button>
  );
}
