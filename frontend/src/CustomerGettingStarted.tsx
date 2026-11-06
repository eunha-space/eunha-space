import type { ReactNode } from "react";
import {
  DoorOpen,
  Info,
  Megaphone,
  ScrollText,
  ShieldCheck,
  Smartphone,
} from "lucide-react";
import { useLingui } from "@lingui/react";
import { SectionCard } from "@/components/customer/page";

type Step = {
  icon: ReactNode;
  title: string;
  body: string;
  /** Pages on the instance itself, by path. */
  links: { label: string; path: string }[];
};

/**
 * What an owner does with a new instance before asking anyone to join, and
 * where on the instance each of those is done. Every path is a page of
 * Eunha's own admin; signing in as the owner is the card above this one.
 */
export function CustomerGettingStarted({ domain }: { domain: string }) {
  const { i18n } = useLingui();
  const t = (message: string, values?: Record<string, string>) =>
    i18n._(message, values);
  const steps: Step[] = [
    {
      icon: <Info />,
      title: t("Introduce your server"),
      body: t(
        "Give it a name, a short description and a contact, so visitors and other servers know who runs it and who it is for.",
      ),
      links: [
        { label: t("Branding"), path: "/admin/settings/branding" },
        { label: t("About page"), path: "/admin/settings/about" },
      ],
    },
    {
      icon: <ScrollText />,
      title: t("Write the rules"),
      body: t(
        "Your rules are shown on the server's about page, and a report can name the rule a post breaks. Add terms of service if you want them too.",
      ),
      links: [
        { label: t("Server rules"), path: "/admin/rules" },
        { label: t("Terms of service"), path: "/admin/terms_of_service" },
      ],
    },
    {
      icon: <DoorOpen />,
      title: t("Decide how people join"),
      body: t(
        "Sign-ups start closed. Open them to everyone, require your approval, or keep them closed and send invite links to the people you want.",
      ),
      links: [
        { label: t("Registrations"), path: "/admin/settings/registrations" },
        { label: t("Invites"), path: "/admin/invites" },
      ],
    },
    {
      icon: <ShieldCheck />,
      title: t("Get ready to moderate"),
      body: t(
        "Reports from members and other servers arrive here. Give trusted members a moderator role, and block servers you don't want to hear from.",
      ),
      links: [
        { label: t("Reports"), path: "/admin/reports" },
        { label: t("Roles"), path: "/admin/roles" },
        { label: t("Blocked servers"), path: "/admin/domain_blocks" },
      ],
    },
    {
      icon: <Megaphone />,
      title: t("Welcome new members"),
      body: t(
        "Post an announcement, suggest accounts to follow, and add custom emoji, so the first visit doesn't land on an empty timeline.",
      ),
      links: [
        { label: t("Announcements"), path: "/admin/announcements" },
        { label: t("Follow recommendations"), path: "/admin/follow_recommendations" },
        { label: t("Custom emoji"), path: "/admin/custom_emojis" },
      ],
    },
    {
      icon: <Smartphone />,
      title: t("Tell people how to sign in"),
      body: t(
        "Members can use the web, or any Mastodon app: in the app, they tap Log in and enter {domain} in the server search.",
        { domain },
      ),
      links: [],
    },
  ];
  return (
    <SectionCard
      title={t("Getting started")}
      description={t(
        "Before you ask anyone to join, a few things are worth setting up. Sign in with the admin account first; each link opens that page on your instance.",
      )}
    >
      <ol className="grid gap-5">
        {steps.map((step) => (
          <li key={step.title} className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1">
            <span className="text-muted-foreground [&_svg]:size-4 pt-0.5">
              {step.icon}
            </span>
            <strong className="text-sm">{step.title}</strong>
            <p className="text-muted-foreground col-start-2 text-sm">{step.body}</p>
            {step.links.length > 0 && (
              <div className="col-start-2 flex flex-wrap gap-x-4 gap-y-1 text-sm">
                {step.links.map((link) => (
                  <a
                    key={link.path}
                    className="text-primary underline-offset-4 hover:underline"
                    href={`https://${domain}${link.path}`}
                    target="_blank"
                    rel="noreferrer"
                  >
                    {link.label}
                  </a>
                ))}
              </div>
            )}
          </li>
        ))}
      </ol>
    </SectionCard>
  );
}
