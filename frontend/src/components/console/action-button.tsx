import type { ComponentProps, ReactNode } from "react";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { Spinner } from "./status";

export type Confirmation = {
  title: string;
  description: string;
  /** Label of the confirming button inside the dialog. */
  action: string;
};

type Props = {
  onClick: () => void;
  busy?: boolean;
  disabled?: boolean;
  /** Shown in a tooltip explaining why the action is unavailable. */
  disabledReason?: string;
  variant?: ComponentProps<typeof Button>["variant"];
  size?: ComponentProps<typeof Button>["size"];
  icon?: ReactNode;
  /** When present, the action runs only after the operator confirms it. */
  confirm?: Confirmation;
  className?: string;
  children: ReactNode;
};

export function ActionButton({
  onClick,
  busy = false,
  disabled = false,
  disabledReason,
  variant = "outline",
  size = "default",
  icon,
  confirm,
  className,
  children,
}: Props) {
  const button = (
    <Button
      variant={variant}
      size={size}
      className={className}
      disabled={busy || disabled}
      onClick={confirm ? undefined : onClick}
    >
      {busy ? <Spinner label="Working" /> : icon}
      {children}
    </Button>
  );

  // A disabled button emits no pointer events, so the tooltip needs a live wrapper.
  const withReason =
    disabled && !busy && disabledReason ? (
      <Tooltip>
        <TooltipTrigger asChild>
          <span tabIndex={0} className="inline-flex cursor-not-allowed">
            {button}
          </span>
        </TooltipTrigger>
        <TooltipContent>{disabledReason}</TooltipContent>
      </Tooltip>
    ) : (
      button
    );

  if (!confirm || disabled || busy) return withReason;

  return (
    <AlertDialog>
      <AlertDialogTrigger asChild>{button}</AlertDialogTrigger>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{confirm.title}</AlertDialogTitle>
          <AlertDialogDescription>{confirm.description}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction onClick={onClick}>{confirm.action}</AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
