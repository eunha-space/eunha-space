import { i18n } from "@lingui/core";
import { I18nProvider, useLingui } from "@lingui/react";
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
  "Loading…": "불러오는 중…",
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
  "Your instances": "내 인스턴스",
  "No instances yet.": "아직 인스턴스가 없습니다.",
  "All instances": "모든 인스턴스",
  Ready: "사용 가능",
  "Setting up": "설정 중",
  "Custom domain setup": "커스텀 도메인 설정",
  "Connect a domain you own to this instance.":
    "보유한 도메인을 이 인스턴스에 연결하세요.",
  "No custom domain was added for this instance.":
    "이 인스턴스에 추가된 커스텀 도메인이 없습니다.",
  Connected: "연결됨",
  "Waiting for DNS": "DNS 설정 대기 중",
  "Your custom domain is connected.": "커스텀 도메인이 연결되었습니다.",
  "Add this CNAME record at your DNS provider.":
    "도메인의 DNS 설정에 다음 CNAME 레코드를 추가하세요.",
  "If requested, also add this verification record.":
    "요청되는 경우 다음 확인 레코드도 추가하세요.",
  "Instance setup": "인스턴스 설정",
  "Your instance is ready to use.": "인스턴스를 사용할 수 있습니다.",
  "This page updates automatically while we set things up.":
    "설정이 진행되는 동안 이 페이지는 자동으로 업데이트됩니다.",
  "Setup complete": "설정 완료",
  "Preparing storage": "저장 공간 준비",
  "Installing the app": "앱 설치",
  "Saving your settings": "설정 저장",
  "Starting your instance": "인스턴스 시작",
  "Checking availability": "사용 가능 여부 확인",
  "Connecting domains": "도메인 연결",
  Done: "완료",
  "In progress": "진행 중",
  "Needs attention": "확인 필요",
  Waiting: "대기 중",
  Type: "유형",
  Value: "값",
  "Eunha hostname": "호스트명",
  Name: "이름",
  "Custom domain (optional)": "커스텀 도메인 (선택)",
  "Point it to customers.eunha.space after creation.":
    "생성 후 customers.eunha.space를 가리키도록 설정하세요.",
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
      <span className="text-xs font-semibold" aria-hidden="true">
        {korean ? "EN" : "KO"}
      </span>
    </Button>
  );
}
