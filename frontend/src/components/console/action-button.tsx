import { useState, type ComponentProps, type ReactNode } from "react";
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
  /**
   * Shows the icon alone and moves `children` into the tooltip and the
   * accessible name, so a dense row of actions costs a column of icons
   * instead of a column of sentences. Requires `icon`.
   */
  iconOnly?: boolean;
  children: ReactNode;
};

/** The accessible name for an icon-only action, from its label children. */
function label(children: ReactNode): string | undefined {
  return typeof children === "string" ? children : undefined;
}

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
  iconOnly = false,
  children,
}: Props) {
  const [open, setOpen] = useState(false);

  const bare = (
    <Button
      variant={variant}
      size={iconOnly && size === "default" ? "icon-sm" : size}
      className={className}
      disabled={busy || disabled}
      onClick={confirm ? undefined : onClick}
      aria-label={iconOnly ? label(children) : undefined}
    >
      {busy ? <Spinner label="Working" /> : icon}
      {iconOnly ? null : children}
    </Button>
  );

  // The label has to stay reachable once it leaves the button face.
  const button = iconOnly && !disabled
    ? (
      <Tooltip>
        <TooltipTrigger render={bare} />
        <TooltipContent>{children}</TooltipContent>
      </Tooltip>
    )
    : bare;

  // A disabled button emits no pointer events, so the tooltip needs a live wrapper.
  if (disabled && !busy && disabledReason) {
    return (
      <Tooltip>
        <TooltipTrigger render={<span tabIndex={0} className="inline-flex cursor-not-allowed" />}>
          {bare}
        </TooltipTrigger>
        <TooltipContent>{disabledReason}</TooltipContent>
      </Tooltip>
    );
  }

  if (!confirm || disabled || busy) return button;

  // A confirming action opens a dialog, so its tooltip hangs off a wrapper
  // rather than the trigger itself.
  const trigger = <AlertDialogTrigger render={bare} />;
  return (
    <AlertDialog open={open} onOpenChange={setOpen}>
      {iconOnly
        ? (
          <Tooltip>
            <TooltipTrigger render={<span className="inline-flex" />}>
              {trigger}
            </TooltipTrigger>
            <TooltipContent>{children}</TooltipContent>
          </Tooltip>
        )
        : trigger}
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{confirm.title}</AlertDialogTitle>
          <AlertDialogDescription>{confirm.description}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          {/* Base UI's action button is a plain button, so closing is explicit. */}
          <AlertDialogAction
            onClick={() => {
              setOpen(false);
              onClick();
            }}
          >
            {confirm.action}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
