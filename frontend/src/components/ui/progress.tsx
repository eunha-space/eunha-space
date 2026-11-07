import { Progress as ProgressPrimitive } from "@base-ui/react/progress";
import { cn } from "cn";
export function Progress({ value, className, ...props }: ProgressPrimitive.Root.Props) {
  return <ProgressPrimitive.Root value={value} className={cn("w-full", className)} {...props}>
    <ProgressPrimitive.Track className="bg-muted h-2 overflow-hidden rounded-full">
      <ProgressPrimitive.Indicator className="bg-primary h-full rounded-full transition-[width]" />
    </ProgressPrimitive.Track>
  </ProgressPrimitive.Root>;
}
