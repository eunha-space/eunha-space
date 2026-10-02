import { i18n } from "@lingui/core";
import { I18nProvider, useLingui } from "@lingui/react";
import { Button } from "@/components/ui/button";

const en: Record<string, string> = {};
const ko: Record<string, string> = {
  "Email delivery": "이메일 발송",
  "SMTP hostname": "SMTP 호스트 이름",
  "Port": "포트",
  "Sender email": "발신 이메일 주소",
  "SMTP username": "SMTP 사용자 이름",
  "SMTP password": "SMTP 비밀번호",
  "Save SMTP settings": "SMTP 설정 저장",
  "Saving…": "저장 중…",
  "Saved. Your instance will apply these settings shortly.": "저장되었습니다. 곧 인스턴스에 설정이 적용됩니다.",
  "Use your SMTP provider for instance emails. Port 465 uses TLS; port 587 requires STARTTLS. Enter both credentials whenever you save; saved credentials are never displayed.": "인스턴스 이메일을 내 SMTP 서비스로 발송합니다. 포트 465는 TLS를, 포트 587은 STARTTLS를 사용합니다. 저장할 때마다 사용자 이름과 비밀번호를 입력하세요. 저장된 인증 정보는 표시되지 않습니다.",
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
  Account: "계정",
  "Set your password": "비밀번호 설정",
  "Set password": "비밀번호 설정",
  "Change your password": "비밀번호 변경",
  "Change password": "비밀번호 변경",
  "Current password": "현재 비밀번호",
  "A new password of at least 14 characters signs out every other device.":
    "14자 이상의 새 비밀번호를 설정하면 다른 모든 기기에서 로그아웃됩니다.",
  "Your password was changed.": "비밀번호가 변경되었습니다.",
  "That current password is not right.": "현재 비밀번호가 올바르지 않습니다.",
  "Forgot your password? Sign in with an email link, then set a new one.":
    "비밀번호를 잊으셨나요? 이메일 링크로 로그인한 뒤 새 비밀번호를 설정하세요.",
  "Your email is verified. Add a password of at least 14 characters for password sign-in.":
    "이메일 인증이 완료되었습니다. 비밀번호 로그인을 위해 14자 이상의 비밀번호를 설정하세요.",
  "Create an instance": "인스턴스 만들기",
  "Create instance": "인스턴스 만들기",
  "You are using every instance your account allows. Delete one to make room for another.":
    "계정에서 허용하는 인스턴스를 모두 사용 중입니다. 새로 만들려면 하나를 삭제하세요.",
  "Creating…": "생성 중…",
  "Your instances": "내 인스턴스",
  "No instances yet.": "아직 인스턴스가 없습니다.",
  "All instances": "모든 인스턴스",
  Ready: "사용 가능",
  "Setting up": "설정 중",
  "Custom domain setup": "커스텀 도메인 설정",
  Connected: "연결됨",
  "Waiting for DNS": "DNS 설정 대기 중",
  "Your custom domain is connected. Keep this CNAME record at your DNS provider; without it, your instance can't be reached at this domain.":
    "커스텀 도메인이 연결되었습니다. DNS 설정에서 이 CNAME 레코드를 유지하세요. 레코드가 없으면 이 도메인으로 인스턴스에 접속할 수 없습니다.",
  "Add this CNAME record at your DNS provider.":
    "도메인의 DNS 설정에 다음 CNAME 레코드를 추가하세요.",
  "If requested, also add this verification record.":
    "요청되는 경우 다음 확인 레코드도 추가하세요.",
  "Instance setup": "인스턴스 설정",
  "Your instance is ready to use.": "인스턴스를 사용할 수 있습니다.",
  "This page updates automatically while we set things up.":
    "설정이 진행되는 동안 이 페이지는 자동으로 업데이트됩니다.",
  "Setup complete": "설정 완료",
  "Preparing your instance": "인스턴스 준비",
  "Starting your instance": "인스턴스 시작",
  "Creating your admin account": "관리자 계정 만들기",
  "Admin username": "관리자 사용자명",
  "Letters, numbers, and underscores.": "영문, 숫자, 밑줄(_)만 사용할 수 있습니다.",
  "Admin account": "관리자 계정",
  "Sign in to your instance as its owner to manage it.":
    "소유자로 인스턴스에 로그인하여 관리하세요.",
  Username: "사용자명",
  "Sign-in email": "로그인 이메일",
  "Sign-in page": "로그인 페이지",
  "Create password": "비밀번호 만들기",
  "Reset password": "비밀번호 재설정",
  "Reset the admin password?": "관리자 비밀번호를 재설정할까요?",
  "A new password is generated, and the admin account is signed out everywhere it is signed in.":
    "새 비밀번호가 만들어지고, 관리자 계정은 로그인된 모든 곳에서 로그아웃됩니다.",
  Cancel: "취소",
  Deleting: "삭제 중",
  Deleted: "삭제됨",
  "Delete instance": "인스턴스 삭제",
  "Restore instance": "인스턴스 복원",
  "Deleted instance": "삭제된 인스턴스",
  "Permanently deleted on": "영구 삭제 예정일",
  "Delete permanently now": "지금 영구 삭제",
  "Delete permanently": "영구 삭제",
  "This instance is offline. Its data is kept until the date below, then permanently deleted unless you restore it.":
    "이 인스턴스는 오프라인 상태입니다. 데이터는 아래 날짜까지 보관되며, 그 전에 복원하지 않으면 영구 삭제됩니다.",
  "Taking this instance offline. Its data is kept for 7 days.":
    "인스턴스를 오프라인으로 전환하는 중입니다. 데이터는 7일 동안 보관됩니다.",
  "Deleting takes this instance offline right away. Its data is kept for 7 days so you can restore it, then permanently deleted.":
    "삭제하면 인스턴스가 즉시 오프라인으로 전환됩니다. 데이터는 7일 동안 보관되어 복원할 수 있으며, 이후 영구 삭제됩니다.",
  "Delete this instance?": "이 인스턴스를 삭제할까요?",
  "It goes offline right away. Its data is permanently deleted after 7 days unless you restore it.":
    "즉시 오프라인으로 전환됩니다. 7일 안에 복원하지 않으면 데이터가 영구 삭제됩니다.",
  "Delete this instance permanently?": "이 인스턴스를 영구 삭제할까요?",
  "Its accounts, posts, media and settings are destroyed now and can't be restored.":
    "계정, 게시물, 미디어, 설정이 지금 삭제되며 복원할 수 없습니다.",
  "Type the instance's address to confirm": "확인하려면 인스턴스 주소를 입력하세요",
  "New password": "새 비밀번호",
  Copy: "복사",
  Copied: "복사됨",
  "Save this password now. It is not stored and will not be shown again.":
    "지금 이 비밀번호를 저장하세요. 저장되지 않으며 다시 표시되지 않습니다.",
  "The password could not be copied. Select it and copy it instead.":
    "비밀번호를 복사하지 못했습니다. 직접 선택하여 복사하세요.",
  Done: "완료",
  "In progress": "진행 중",
  "Needs attention": "확인 필요",
  Waiting: "대기 중",
  Type: "유형",
  Value: "값",
  "Eunha hostname": "호스트명",
  Name: "이름",
  "Instance name": "인스턴스 이름",
  "No instance limit.": "인스턴스 수 제한이 없습니다.",
  Address: "주소",
  "eunha space address": "은하 도메인",
  "My own domain": "내 도메인",
  "Custom domain": "커스텀 도메인",
  "Before creating, add a CNAME record pointing it to customers.eunha.space. You can move the instance to another domain later.":
    "생성하기 전에 customers.eunha.space를 가리키는 CNAME 레코드를 추가하세요. 나중에 인스턴스를 다른 도메인으로 옮길 수 있습니다.",
  "We can't see that CNAME record yet. You can still create the instance; setup keeps trying for 7 days, then deletes the instance.":
    "아직 CNAME 레코드가 확인되지 않습니다. 인스턴스는 만들 수 있으며, 설정은 7일 동안 계속 시도한 뒤 인스턴스를 삭제합니다.",
  "The CNAME record is in place.": "CNAME 레코드가 확인되었습니다.",
  "Keep each domain's DNS record in place for as long as it is connected.":
    "도메인이 연결되어 있는 동안 DNS 레코드를 그대로 유지하세요.",
  Removing: "제거 중",
  "This domain is being released.": "이 도메인을 해제하고 있습니다.",
  Domain: "도메인",
  "Your instance's address. Every account's handle and every post's link are under it.":
    "인스턴스의 주소입니다. 모든 계정의 핸들과 모든 게시물의 링크가 이 주소 아래에 있습니다.",
  "Change domain": "도메인 변경",
  "Move to this domain": "이 도메인으로 옮기기",
  "Move your instance to": "인스턴스를 다음 주소로 옮길까요:",
  "Every account's followers are asked to follow it at the new domain, and your accounts follow again whoever they follow. Posts move too, but under new links: links to old posts, and replies to them from other servers, stop working. The current domain stops answering once the move begins, and this can't be undone.":
    "모든 계정의 팔로워에게 새 도메인에서 계정을 팔로우하도록 요청하고, 계정이 팔로우하던 사람들을 새 도메인에서 다시 팔로우합니다. 게시물도 옮겨지지만 링크가 바뀌므로, 이전 게시물로 가는 링크와 다른 서버에서 그 게시물에 단 답글은 더 이상 동작하지 않습니다. 이동이 시작되면 현재 도메인은 응답하지 않으며, 되돌릴 수 없습니다.",
  "Add a CNAME record pointing it to customers.eunha.space. Nothing moves until it's connected, and you can stop until then.":
    "customers.eunha.space를 가리키는 CNAME 레코드를 추가하세요. 연결되기 전까지는 아무것도 옮겨지지 않으며, 그때까지는 이동을 멈출 수 있습니다.",
  "Connecting the new domain": "새 도메인 연결",
  "Moving your instance": "인스턴스 옮기기",
  "Moving your followers": "팔로워 옮기기",
  "Releasing the old domain": "이전 도메인 해제",
  "servers to go": "개 서버 남음",
  "Stop the move": "이동 멈추기",
  "Moved from": "이전 주소:",
  "servers didn't take the move; their users can follow your accounts again by hand.":
    "개 서버가 이동을 받지 못했습니다. 그 서버의 사용자는 직접 다시 팔로우할 수 있습니다.",
  "The move didn't start: the new domain couldn't be connected. Your instance stays where it is.":
    "새 도메인을 연결하지 못해 이동을 시작하지 않았습니다. 인스턴스는 그대로 있습니다.",
  "The move stopped part of the way. We've been told and will finish it.":
    "이동이 도중에 멈췄습니다. 운영팀에 전달되었으며 이동을 마무리할 예정입니다.",
  "If it isn't connected by then, this instance is deleted":
    "이때까지 연결되지 않으면 인스턴스가 삭제됩니다",
  "This instance was deleted because its custom domain wasn't connected within 7 days, so it was never set up. Restore it to try the domain again.":
    "커스텀 도메인이 7일 안에 연결되지 않아 인스턴스가 설정되지 않은 채 삭제되었습니다. 복원하면 다시 도메인 연결을 시도합니다.",
  "Its DNS doesn't point to customers.eunha.space yet. We'll keep checking.":
    "아직 DNS가 customers.eunha.space를 가리키지 않습니다. 계속 확인하겠습니다.",
  "We couldn't finish checking this domain yet. We'll keep trying.":
    "아직 이 도메인 확인을 마치지 못했습니다. 계속 시도하겠습니다.",
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
