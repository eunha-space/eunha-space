import * as React from "react";
import { ChevronDown } from "lucide-react";
import { cn } from "cn";

function NativeSelect({ className, ...props }: React.ComponentProps<"select">) {
  return <div data-slot="native-select-wrapper" className="group/native-select relative w-fit has-disabled:opacity-50">
    <select data-slot="native-select" className={cn("border-input bg-background text-foreground focus-visible:border-ring focus-visible:ring-ring/50 h-8 w-full min-w-0 appearance-none rounded-lg border py-1 pr-8 pl-3 text-sm outline-none focus-visible:ring-3 disabled:pointer-events-none", className)} {...props} />
    <ChevronDown aria-hidden="true" className="text-muted-foreground pointer-events-none absolute top-1/2 right-2 size-4 -translate-y-1/2" />
  </div>;
}

export { NativeSelect };
