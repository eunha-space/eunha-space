import { i18n } from "@lingui/core";
import { I18nProvider, useLingui } from "@lingui/react";
import { Languages } from "lucide-react";
import { Button } from "@/components/ui/button";

const en: Record<string, string> = {};
const ko: Record<string, string> = {
  "Your eunha space instances": "나의 eunha space 인스턴스",
  "Create your account": "계정 만들기",
  "Sign in to create and manage your instances.":
    "로그인하여 인스턴스를 만들고 관리하세요.",
  "Verify your email before creating up to two instances.":
    "이메일 인증 후 최대 2개의 인스턴스를 만들 수 있습니다.",
  "Check your inbox and follow the verification link.":
    "받은편지함에서 인증 링크를 확인하세요.",
  "Email verified. Sign in to continue.":
    "이메일 인증이 완료되었습니다. 계속하려면 로그인하세요.",
  Email: "이메일",
  Password: "비밀번호",
  "Create account": "계정 만들기",
  "Sign in": "로그인",
  "Please wait…": "잠시만 기다려 주세요…",
  "Email me a sign-in link": "이메일로 로그인 링크 받기",
  "Already have an account? Sign in": "이미 계정이 있나요? 로그인",
  "New here? Create an account": "처음이신가요? 계정 만들기",
  "Set your password": "비밀번호 설정",
  "Set password": "비밀번호 설정",
  "Your email is verified. Add a password of at least 14 characters for password sign-in.":
    "이메일 인증이 완료되었습니다. 비밀번호 로그인을 위해 14자 이상의 비밀번호를 설정하세요.",
  "Create an instance": "인스턴스 만들기",
  "Create instance": "인스턴스 만들기",
  "Creating…": "생성 중…",
  "{used} of {limit} used. Runtime placement is automatic.":
    "{limit}개 중 {used}개 사용 중입니다. 런타임 배치는 자동입니다.",
  "Your instances": "내 인스턴스",
  "No instances yet.": "아직 인스턴스가 없습니다.",
  "Eunha hostname": "Eunha 호스트명",
  Name: "이름",
  "Custom domain (optional)": "사용자 도메인 (선택)",
  "Point it to customers.eunha.space after creation.":
    "생성 후 customers.eunha.space를 가리키도록 설정하세요.",
  "Provisioning continues safely in the background.":
    "프로비저닝은 백그라운드에서 안전하게 계속됩니다.",
  "Runtime placement is automatic.": "런타임 배치는 자동입니다.",
  "Use a password with at least 14 characters.":
    "14자 이상의 비밀번호를 사용하세요.",
  "Control plane": "제어판",
  "Sign in to manage the Eunha runtime fleet.":
    "Eunha 런타임을 관리하려면 로그인하세요.",
  Operator: "운영자",
  "Signing in…": "로그인 중…",
  Tenants: "테넌트",
  Instances: "인스턴스",
  Releases: "릴리스",
  Operations: "작업",
  "Sign out": "로그아웃",
  "Refresh now": "새로고침",
  "Runtime fleet": "런타임 플릿",
};
i18n.load("en", en);
i18n.load("ko", ko);
const initial = localStorage.getItem("locale") ||
  (navigator.language.startsWith("ko") ? "ko" : "en");
i18n.activate(initial);
document.documentElement.lang = initial;

export function LocalizationProvider(
  { children }: { children: React.ReactNode },
) {
  return <I18nProvider i18n={i18n}>{children}</I18nProvider>;
}
export function LanguageToggle() {
  const { i18n } = useLingui();
  const korean = i18n.locale === "ko";
  return (
    <Button
      variant="outline"
      size="icon"
      onClick={() => {
        const next = korean ? "en" : "ko";
        localStorage.setItem("locale", next);
        document.documentElement.lang = next;
        i18n.activate(next);
      }}
      aria-label={korean ? "Switch to English" : "한국어로 전환"}
      title={korean ? "English" : "한국어"}
    >
      <Languages />
    </Button>
  );
}
