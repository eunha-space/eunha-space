import { i18n } from "@lingui/core";
import { compileMessage } from "@lingui/message-utils/compileMessage";
import { takeBoot } from "@/api";
import { I18nProvider, useLingui } from "@lingui/react";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";

const en: Record<string, string> = {};
const ko: Record<string, string> = {
  "Port 465 uses TLS; port 587 requires STARTTLS.": "포트 465는 TLS를, 포트 587은 STARTTLS를 사용합니다.",
  "Enter both credentials whenever you save; saved credentials are never displayed.": "저장할 때마다 사용자 이름과 비밀번호를 입력하세요. 저장된 인증 정보는 표시되지 않습니다.",
  "Refund policy": "환불 정책",
  "Community guidelines": "커뮤니티 가이드라인",
  "Billing terms": "결제 안내",
  "Open instance": "인스턴스 열기",
  "No payments yet.": "아직 결제 내역이 없습니다.",
  "Refunds are available for 7 days after a payment.": "결제 후 7일 동안 환불할 수 있습니다.",
  Site: "사이트",
  "Switch to the light theme": "밝은 테마로 전환",
  "Switch to the dark theme": "어두운 테마로 전환",
  "Custom domains require your own SMTP credentials. Email delivery is unavailable until you save them.": "커스텀 도메인은 직접 SMTP 인증 정보를 설정해야 합니다. 저장하기 전까지 이메일을 발송할 수 없습니다.",
  "Email delivery": "이메일 발송",
  "SMTP hostname": "SMTP 호스트 이름",
  "Port": "포트",
  "Sender email": "발신 이메일 주소",
  "SMTP username": "SMTP 사용자 이름",
  "SMTP password": "SMTP 비밀번호",
  "Save SMTP settings": "SMTP 설정 저장",
  "Saving…": "저장 중…",
  "Saved. Your instance will apply these settings shortly.": "저장되었습니다. 곧 인스턴스에 설정이 적용됩니다.",
  "eunha space": "은하공간",
  "Your own Mastodon-compatible server, run for you.":
    "마스토돈 호환 서버: 은하 호스팅은 은하공간에 맡기세요",
  "eunha space hosts Eunha, a Mastodon-compatible fediverse server. Choose an address and you have a server of your own, where you can follow and be followed by people on Mastodon and across the fediverse.":
    "은하공간은 마스토돈과 호환되는 연합우주(페디버스) 서버 은하를 호스팅하는 서비스입니다. 주소만 정하면 나만의 서버가 생기고, 마스토돈을 비롯한 연합우주 곳곳의 사람들과 서로 팔로우할 수 있습니다.",
  "How it works": "이용 방법",
  "Your server": "나의 서버",
  "What is this?": "무엇인가요?",
  "The fediverse": "연합우주(페디버스)",
  "A network of independently run social servers that talk to each other over an open protocol, ActivityPub. Mastodon is the best known of them. It works like email: whichever server your account is on, you can reach people on the others.":
    "각자 독립적으로 운영되는 소셜 서버들이 ActivityPub이라는 공개 프로토콜로 서로 소통하는 네트워크입니다. 그중 가장 잘 알려진 것이 마스토돈입니다. 이메일처럼 계정이 어느 서버에 있든 다른 서버의 사람들과 소통할 수 있습니다.",
  Eunha: "은하",
  "Free, open-source server software that reimplements Mastodon in Rust. It tracks each Mastodon release and works with Mastodon's apps.":
    "마스토돈을 Rust로 다시 구현한 오픈 소스 서버 소프트웨어입니다. 마스토돈의 새 릴리스를 계속 따라가며, 마스토돈 및 호환 앱을 그대로 쓸 수 있습니다.",
  "Hosting for Eunha. We run the server and keep it up to date, so you can spend your time on your community instead.":
    "은하 호스팅 서비스입니다. 서버 운영과 업데이트는 저희가 맡으니, 커뮤니티에만 집중하세요.",
  "What you get": "제공 내용",
  "An address of your own": "나만의 주소",
  "Start with a name under eunha.space, or use a domain you own by pointing a CNAME at us. You can move to another domain later, and your followers are told where you went.":
    "eunha.space의 하위 주소로 시작하거나, CNAME을 연결해 내 도메인을 쓸 수 있습니다. 나중에 다른 도메인으로 옮겨도 팔로워에게 새 주소가 전달됩니다.",
  "Your server, your rules": "내 서버, 내 규칙",
  "You are the server's owner, with its tools for members, moderation and settings. Registrations start closed, so you decide who joins.":
    "서버의 소유자로서 회원 관리, 운영, 설정 도구를 모두 쓸 수 있습니다. 처음에는 가입이 닫혀 있어 누구를 받을지 직접 정할 수 있습니다.",
  "The Mastodon apps you know": "익숙한 마스토돈 앱 그대로",
  "Eunha speaks Mastodon's API, so you can keep using the Mastodon apps and clients you like.":
    "은하는 마스토돈 API를 제공하므로, 좋아하는 마스토돈 앱과 클라이언트를 계속 쓸 수 있습니다.",
  "Free to leave": "언제든 떠날 자유",
  "Members can export their data and move their accounts, followers included, to another server. Eunha itself is open source under the AGPL.":
    "회원은 데이터를 내보내고, 팔로워와 함께 계정을 다른 서버로 옮길 수 있습니다. 은하는 AGPL 라이선스의 오픈 소스입니다.",
  "Create an account": "계정 만들기",
  "All you need is an email address. Follow the link we send you.":
    "이메일 주소만 있으면 됩니다. 보내 드리는 링크를 따라가세요.",
  "Choose a monthly plan and pay by card.": "월간 요금제를 고르고 카드로 결제하세요.",
  "Create your instance": "인스턴스 만들기",
  "Pick an address and an admin username. Setup runs by itself, and with your own domain it starts once the domain validates.":
    "주소와 관리자 사용자 이름을 정하세요. 설정은 자동으로 진행되며, 내 도메인을 쓰는 경우 도메인이 확인된 뒤 시작됩니다.",
  "Sign in to your server": "내 서버에 로그인",
  "Create your admin password on the instance page, then sign in at your new address.":
    "인스턴스 페이지에서 관리자 비밀번호를 만든 뒤, 새 주소에서 로그인하세요.",
  Pricing: "요금",
  "/ month": "/ 월",
  "Every plan includes hosting, automatic updates and an address of your own.":
    "모든 요금제에 호스팅, 자동 업데이트, 나만의 주소가 포함됩니다.",
  Custom: "맞춤",
  "For bigger instances, we'll work out a plan with you.":
    "더 큰 인스턴스를 위한 요금제를 함께 정해 드립니다.",
  "Contact us": "문의하기",
  "Every plan is charged monthly.": "모든 요금제는 매월 결제됩니다.",
  "A payment can be refunded in full for 7 days after it is made, and not after that.":
    "결제 후 7일 이내에는 결제 금액의 100%를 환불받을 수 있으며, 7일이 지나면 환불되지 않습니다.",
  "With an international card, only your first payment can be refunded from your account page; write to us about any other.":
    "해외 카드로 결제한 경우 계정 페이지에서는 첫 결제만 환불받을 수 있으며, 그 밖의 결제는 문의해 주세요.",
  "Cancel any time and keep the period you already paid for.":
    "언제든지 해지할 수 있으며, 이미 결제한 기간은 그대로 이용할 수 있습니다.",
  "If a subscription ends, your instances go offline and their data is kept. Subscribing again brings them back.":
    "구독이 끝나면 인스턴스는 오프라인이 되지만 데이터는 보관됩니다. 다시 구독하면 되살아납니다.",
  "A deleted instance can be restored for seven days.":
    "삭제한 인스턴스는 7일 동안 복원할 수 있습니다.",
  "About Eunha": "은하 소개",
  "Source code": "소스 코드",
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
  "Sign out": "로그아웃",
  "Control plane": "컨트롤 플레인",
  "You are signed in as {email}.": "{email} 계정으로 로그인한 상태입니다.",
  "Back to the control plane": "컨트롤 플레인으로 돌아가기",
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
  "You are using every instance your account allows. Delete one to make room for another.":
    "계정에서 허용하는 인스턴스를 모두 사용 중입니다. 새로 만들려면 하나를 삭제하세요.",
  "Creating…": "생성 중…",
  "Your instances": "내 인스턴스",
  "No instances yet.": "아직 인스턴스가 없습니다.",
  Ready: "사용 가능",
  "Setting up": "설정 중",
  "Custom domain setup": "커스텀 도메인 설정",
  Connected: "연결됨",
  "Waiting for DNS": "DNS 설정 대기 중",
  "Add this CNAME record at your DNS provider.":
    "도메인의 DNS 설정에 다음 CNAME 레코드를 추가하세요.",
  "If requested, also add this verification record.":
    "요청되는 경우 다음 확인 레코드도 추가하세요.",
  "Instance setup": "인스턴스 설정",
  "Your instance is ready to use.": "인스턴스를 사용할 수 있습니다.",
  "This page updates automatically while we set things up.":
    "설정이 진행되는 동안 이 페이지는 자동으로 업데이트됩니다.",
  "Preparing your instance": "인스턴스 준비",
  "Starting your instance": "인스턴스 시작",
  "Creating your admin account": "관리자 계정 만들기",
  "Admin username": "관리자 사용자명",
  "Letters, numbers, and underscores.": "영문, 숫자, 밑줄(_)만 사용할 수 있습니다.",
  "Admin account": "관리자 계정",
  "Getting started": "시작하기",
  "Before you ask anyone to join, a few things are worth setting up. Sign in with the admin account first; each link opens that page on your instance.":
    "다른 분들을 초대하기 전에 몇 가지를 준비해 두면 좋습니다. 먼저 관리자 계정으로 로그인하세요. 각 링크를 누르면 인스턴스의 해당 페이지가 열립니다.",
  "Introduce your server": "서버 소개하기",
  "Give it a name, a short description and a contact, so visitors and other servers know who runs it and who it is for.":
    "서버 이름과 짧은 소개, 연락처를 적어 두세요. 방문자와 다른 서버가 누가 운영하는 어떤 서버인지 알 수 있습니다.",
  Branding: "브랜딩",
  "About page": "소개 페이지",
  "Write the rules": "규칙 정하기",
  "Your rules are shown on the server's about page, and a report can name the rule a post breaks. Add terms of service if you want them too.":
    "규칙은 서버 소개 페이지에 표시되고, 신고할 때 어떤 규칙을 어겼는지 고를 수 있습니다. 필요하면 이용약관도 추가하세요.",
  "Server rules": "서버 규칙",
  "Terms of service": "이용약관",
  "Decide how people join": "가입 방식 정하기",
  "Sign-ups start closed. Open them to everyone, require your approval, or keep them closed and send invite links to the people you want.":
    "처음에는 가입이 닫혀 있습니다. 누구나 가입하게 열거나, 승인을 거치게 하거나, 닫아 둔 채 원하는 분들께 초대 링크를 보낼 수 있습니다.",
  Registrations: "가입 설정",
  Invites: "초대",
  "Get ready to moderate": "운영 준비하기",
  "Reports from members and other servers arrive here. Give trusted members a moderator role, and block servers you don't want to hear from.":
    "회원과 다른 서버에서 보낸 신고가 이곳에 모입니다. 믿을 수 있는 회원에게 모더레이터 역할을 주고, 원하지 않는 서버는 차단하세요.",
  Reports: "신고",
  Roles: "역할",
  "Blocked servers": "서버 차단",
  "Welcome new members": "새 회원 맞이하기",
  "Post an announcement, suggest accounts to follow, and add custom emoji, so the first visit doesn't land on an empty timeline.":
    "공지를 올리고, 팔로우할 만한 계정을 추천하고, 커스텀 이모지를 추가해 두세요. 처음 온 분이 빈 타임라인을 마주하지 않도록요.",
  Announcements: "공지",
  "Follow recommendations": "팔로우 추천",
  "Custom emoji": "커스텀 이모지",
  "Tell people how to sign in": "로그인 방법 알려 주기",
  "Members can use the web, or any Mastodon app: in the app, they tap Log in and enter {domain} in the server search.":
    "회원은 웹이나 마스토돈 앱으로 이용할 수 있습니다. 앱에서는 로그인을 누른 뒤 서버 검색 칸에 {domain} 주소를 입력하면 됩니다.",
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
  "Configure site settings inside Eunha after setup.": "설정이 완료되면 은하에서 인스턴스 설정을 변경하세요.",
  "No instance limit.": "인스턴스 수 제한이 없습니다.",
  Address: "주소",
  "eunha space address": "은하공간 주소",
  "My own domain": "내 도메인",
  "Custom domain": "커스텀 도메인",
  "Before creating, add a CNAME record pointing it to customers.eunha.space. You can move the instance to another domain later.":
    "생성하기 전에 customers.eunha.space를 가리키는 CNAME 레코드를 추가하세요. 나중에 인스턴스를 다른 도메인으로 옮길 수 있습니다.",
  "We can't see that CNAME record yet. You can still create the instance; setup keeps trying for 7 days, then deletes the instance.":
    "아직 CNAME 레코드가 확인되지 않습니다. 인스턴스는 만들 수 있으며, 설정은 7일 동안 계속 시도한 뒤 인스턴스를 삭제합니다.",
  "The CNAME record is in place.": "CNAME 레코드가 확인되었습니다.",
  "Handle domain (optional)": "핸들 도메인 (선택)",
  "For handles on another domain you own, such as @admin@example.com while the instance is served on social.example.com. That domain must redirect /.well-known/webfinger to the instance, and neither domain can be changed afterwards.":
    "인스턴스는 social.example.com에서 운영하면서 핸들은 @admin@example.com처럼 소유한 다른 도메인으로 쓰려면 입력하세요. 그 도메인은 /.well-known/webfinger를 인스턴스로 리디렉션해야 하며, 이후에는 두 도메인 모두 바꿀 수 없습니다.",
  Redirecting: "리디렉션 중",
  "Waiting for the redirect": "리디렉션 대기 중",
  "Handles on this instance end in @{domain}. Keep this redirect in place: other servers find accounts through it.":
    "이 인스턴스의 핸들은 @{domain}으로 끝납니다. 다른 서버가 이 리디렉션으로 계정을 찾으므로 유지해 주세요.",
  "Handles on this instance end in @{domain}. Have the web server for {domain} redirect /.well-known/webfinger here; with nginx:":
    "이 인스턴스의 핸들은 @{domain}으로 끝납니다. {domain}의 웹 서버가 /.well-known/webfinger를 이 인스턴스로 리디렉션하도록 설정하세요. nginx의 경우:",
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

  "This instance is offline because your subscription ended. Its data is kept, and subscribing again serves it again.":
    "구독이 종료되어 이 인스턴스가 오프라인 상태입니다. 데이터는 보관되며, 다시 구독하면 다시 서비스됩니다.",
  "The card could not be registered.":
    "카드를 등록하지 못했습니다.",
  "Your subscription is cancelled. Your instances keep running until the end of the paid period, then go offline with their data kept.":
    "구독이 해지되었습니다. 결제한 기간이 끝날 때까지 인스턴스가 계속 운영되며, 이후 데이터는 보관된 채 오프라인으로 전환됩니다.",
  "Your card is charged automatically each month.":
    "매달 등록된 카드로 자동 결제됩니다.",
  "Your last payment was declined. We'll try again, and your instances keep running meanwhile. Use another card to pay now.":
    "최근 결제가 거절되었습니다. 다시 결제를 시도하며, 그동안 인스턴스는 계속 운영됩니다. 지금 결제하려면 다른 카드를 등록하세요.",
  "Your payment is being confirmed.":
    "결제를 확인하고 있습니다.",
  "Your subscription has ended, so your instances are offline with their data kept. Subscribe again to serve them again.":
    "구독이 종료되어 인스턴스가 오프라인 상태입니다. 데이터는 보관되며, 다시 구독하면 다시 서비스됩니다.",
  "Subscribe to create and run instances.":
    "인스턴스를 만들고 운영하려면 구독하세요.",
  "Subscription":
    "구독",
  "Active":
    "이용 중",
  "Cancelled":
    "해지됨",
  "Payment declined":
    "결제 거절",
  "Monthly price":
    "월 요금",
  "Card":
    "카드",
  "Ends on":
    "종료일",
  "Renews on":
    "다음 결제일",
  "Next attempt":
    "다음 결제 시도",
  "Subscribe with a card":
    "카드로 구독하기",
  "Subscribe with a Korean card":
    "국내 카드로 구독하기",
  "Pay with an international card":
    "해외 카드로 결제하기",
  "With an international card":
    "해외 카드 요금",
  "International cards:":
    "해외 카드:",
  "Manage billing":
    "결제 관리",
  "Your international card is charged automatically each month through Polar, which sends your receipts.":
    "매달 Polar를 통해 해외 카드로 자동 결제되며, 영수증은 Polar가 보내드립니다.",
  "Your last payment was declined. Polar will try again, and your instances keep running meanwhile. Update your card in the billing portal.":
    "최근 결제가 거절되었습니다. Polar가 다시 결제를 시도하며, 그동안 인스턴스는 계속 운영됩니다. 결제 관리에서 카드를 변경하세요.",
  "You are on a free trial until":
    "무료 체험 종료일:",
  "Polar charges your international card then, and each month after. Cancel before then and you pay nothing.":
    "그때부터 매월 Polar가 해외 카드로 결제합니다. 그 전에 해지하면 요금이 청구되지 않습니다.",
  "New subscriptions are paused for now. Please try again later.":
    "지금은 새 구독을 받지 않고 있습니다. 나중에 다시 시도해 주세요.",
  "With a Korean card":
    "국내 카드 요금",
  "Full refund within 7 days":
    "7일 이내 전액 환불",
  "Korean cards: full refund within 7 days":
    "국내 카드: 7일 이내 전액 환불",
  "First charge on":
    "첫 결제일",
  "It starts now, and your free trial carries on until":
    "지금 바로 적용되며, 무료 체험은 다음 날짜까지 이어집니다:",
  "Nothing is charged before then.":
    "그 전에는 결제되지 않습니다.",
  "It starts now and ends your free trial, because this plan's trial is shorter or there is none. Polar charges your card this plan's price now:":
    "이 요금제는 체험 기간이 더 짧거나 없어서, 지금 바로 적용되며 무료 체험이 끝납니다. Polar가 이 요금제의 요금을 지금 카드로 결제합니다:",
  "It starts now, and Polar charges your card the difference for the rest of this period.":
    "지금 바로 적용되며, 남은 기간에 대한 차액을 Polar가 카드로 결제합니다.",
  "This subscription is paid by an international card. Manage it in the billing portal.":
    "이 구독은 해외 카드로 결제되고 있습니다. 결제 관리에서 변경하세요.",
  "International cards are not available.":
    "해외 카드 결제를 이용할 수 없습니다.",
  "This plan is not available with an international card.":
    "이 요금제는 해외 카드로 결제할 수 없습니다.",
  "This account does not pay through Polar.":
    "이 계정은 Polar로 결제하고 있지 않습니다.",
  "Polar could not be reached. Try again shortly.":
    "Polar에 연결하지 못했습니다. 잠시 후 다시 시도하세요.",
  "Polar could not make this refund. Please contact us.":
    "Polar에서 환불을 처리하지 못했습니다. 문의해 주세요.",
  "The last payment failed. Update your card in the billing portal.":
    "최근 결제에 실패했습니다. 결제 관리에서 카드를 변경하세요.",
  "Use another card":
    "다른 카드 사용",
  "Change card":
    "카드 변경",
  "Cancel subscription":
    "구독 해지",
  "Keep my subscription":
    "구독 유지",
  "Payments":
    "결제 내역",
  "Period":
    "기간",
  "Amount":
    "금액",
  "Status":
    "상태",
  "Paid":
    "결제 완료",
  "Confirming":
    "확인 중",
  "Declined":
    "거절됨",
  "Cancel your subscription?":
    "구독을 해지할까요?",
  "Your paid period is already over, so your instances go offline now. Their data is kept, and subscribing again serves them again.":
    "결제한 기간이 이미 끝났으므로 인스턴스가 지금 오프라인으로 전환됩니다. 데이터는 보관되며, 다시 구독하면 다시 서비스됩니다.",
  "Your instances keep running until the end of the paid period, then go offline. Their data is kept, and subscribing again serves them again.":
    "결제한 기간이 끝날 때까지 인스턴스가 계속 운영되고, 이후 오프라인으로 전환됩니다. 데이터는 보관되며, 다시 구독하면 다시 서비스됩니다.",
  "Subscribe":
    "구독하기",
  "Subscribe to run an instance.":
    "인스턴스를 운영하려면 구독하세요.",
  "Subscriptions are not available.":
    "지금은 구독할 수 없습니다.",
  "TossPayments could not be reached. Try again shortly.":
    "토스페이먼츠에 연결하지 못했습니다. 잠시 후 다시 시도하세요.",
  "There is no subscription to cancel.":
    "해지할 구독이 없습니다.",
  "This subscription has ended. Subscribe again to continue.":
    "구독이 이미 종료되었습니다. 계속하려면 다시 구독하세요.",
  "The payment was not approved.":
    "결제가 승인되지 않았습니다.",
  "The payment was canceled.":
    "결제가 취소되었습니다.",
  "Plan": "요금제",
  "Choose a plan": "요금제 선택",
  "Change plan": "요금제 변경",
  "Current plan": "현재 요금제",
  "From your next renewal": "다음 갱신부터",
  "Change to this plan?": "이 요금제로 변경할까요?",
  "Keep this plan": "변경하지 않기",
  "It starts now, and we charge your card the difference for the rest of this period:":
    "지금 바로 적용되며, 이번 이용 기간의 남은 기간에 대한 차액을 카드로 결제합니다:",
  "Your renewal date stays the same.": "갱신일은 그대로입니다.",
  "It starts now at no charge, because so little of this period is left.":
    "이번 이용 기간이 거의 끝나 추가 요금 없이 지금 바로 적용됩니다.",
  "It starts at your next renewal. Nothing changes until then, and you can change your mind.":
    "다음 갱신일부터 적용됩니다. 그때까지는 달라지는 것이 없으며, 언제든 되돌릴 수 있습니다.",
  "You stay on your current plan, and the change waiting for your next renewal is dropped.":
    "현재 요금제를 유지하며, 다음 갱신일에 예정된 변경은 취소됩니다.",
  "This plan is not available.": "이 요금제는 선택할 수 없습니다.",
  "Only an active subscription can change plans.": "이용 중인 구독만 요금제를 변경할 수 있습니다.",
  "Keep your subscription before changing plans.": "요금제를 변경하려면 먼저 구독을 유지해 주세요.",
  "Delete instances until you are within this plan's limit first.":
    "이 요금제의 한도에 맞게 인스턴스를 먼저 삭제해 주세요.",
  "Add a card before upgrading.": "요금제를 올리려면 먼저 카드를 등록해 주세요.",
  "Refund":
    "환불",
  "Refunded":
    "환불됨",
  "Refund pending":
    "환불 처리 중",
  "Refund refused":
    "환불 거절",
  "Refund this payment?":
    "이 결제를 환불할까요?",
  "The full amount goes back to your card. Your subscription is cancelled and your instances go offline immediately. Their data is kept, and subscribing again serves them again.":
    "전액이 카드로 환불되고 구독이 해지되며 인스턴스는 즉시 오프라인으로 전환됩니다. 데이터는 보관되며, 다시 구독하면 다시 서비스됩니다.",
  "This payment can no longer be refunded.":
    "이 결제는 더 이상 환불할 수 없습니다.",
  "A payment is being confirmed. Try again shortly.":
    "결제를 확인하고 있습니다. 잠시 후 다시 시도하세요.",
  "Payment not found.":
    "결제 내역을 찾을 수 없습니다.",
  "This payment was taken under an earlier payment setup and cannot be refunded here. Please contact us.":
    "이전 결제 설정으로 이루어진 결제라 여기서 환불할 수 없습니다. 문의해 주세요.",
  "This payment cannot be refunded here. Please contact us.":
    "이 결제는 여기서 환불할 수 없습니다. 문의해 주세요.",
  "Language":
    "언어",
  "Choose the language for this site and the emails we send you.":
    "이 사이트와 보내 드리는 이메일의 언어를 선택하세요.",
  "Choose English or Korean.":
    "영어나 한국어를 선택하세요.",
  // Messages the server answers customers with; ErrorAlert looks them up.
  "Too many attempts. Try again in 15 minutes.":
    "시도 횟수가 너무 많습니다. 15분 뒤에 다시 시도하세요.",
  "Password is too long.": "비밀번호가 너무 깁니다.",
  "Password service is busy. Try again shortly.":
    "비밀번호 확인이 지연되고 있습니다. 잠시 뒤에 다시 시도하세요.",
  "Client address unavailable.": "접속 주소를 확인할 수 없습니다.",
  "Sign in required": "로그인이 필요합니다.",
  "Session expired": "세션이 만료되었습니다. 다시 로그인하세요.",
  "Enter a valid email address.": "올바른 이메일 주소를 입력하세요.",
  "Invalid email address.": "올바르지 않은 이메일 주소입니다.",
  "An account with that email already exists.": "이미 그 이메일로 만든 계정이 있습니다.",
  "Customer email verification is not configured.":
    "이메일 인증이 설정되어 있지 않습니다. 문의해 주세요.",
  "Could not build the email link.": "이메일 링크를 만들지 못했습니다.",
  "Could not send the email link.": "이메일 링크를 보내지 못했습니다.",
  "That verification link is invalid or expired.":
    "인증 링크가 올바르지 않거나 만료되었습니다.",
  "Invalid credentials": "이메일 주소나 비밀번호가 올바르지 않습니다.",
  "Set a password after signing in with an email link.":
    "이메일 링크로 로그인한 뒤 비밀번호를 설정하세요.",
  "Verify your email before signing in.": "로그인하기 전에 이메일 인증을 완료하세요.",
  "Instance not found": "인스턴스를 찾을 수 없습니다.",
  "Set a password before creating an instance.":
    "인스턴스를 만들기 전에 비밀번호를 설정하세요.",
  "Use up to 30 letters, numbers, and underscores for the admin username.":
    "관리자 사용자 이름은 영문자, 숫자, 밑줄(_)로 30자까지 쓸 수 있습니다.",
  "No runtime is currently available.":
    "지금은 인스턴스를 만들 수 없습니다. 잠시 뒤에 다시 시도하세요.",
  "Enter a valid externally owned custom domain.":
    "직접 소유한 올바른 커스텀 도메인을 입력하세요.",
  "Enter a valid domain.": "올바른 도메인을 입력하세요.",
  "No provided domain suffix is configured.":
    "eunha.space 주소를 지금은 쓸 수 없습니다. 문의해 주세요.",
  "That subdomain is reserved.": "예약된 하위 주소입니다.",
  "Cloudflare for SaaS is not configured.":
    "커스텀 도메인을 지금은 쓸 수 없습니다. 문의해 주세요.",
  "You have reached your instance limit.": "만들 수 있는 인스턴스 수에 도달했습니다.",
  "That domain is already managed.": "이미 사용 중인 도메인입니다.",
  "Could not generate tenant keys.": "인스턴스 키를 만들지 못했습니다.",
  "This instance is already deleted.": "이미 삭제된 인스턴스입니다.",
  "Only a deleted instance can be deleted permanently.":
    "삭제된 인스턴스만 영구 삭제할 수 있습니다.",
  "Type the instance's address to confirm.": "확인을 위해 인스턴스 주소를 입력하세요.",
  "Only a deleted instance can be restored.": "삭제된 인스턴스만 복원할 수 있습니다.",
  "Delete another instance before restoring this one.":
    "이 인스턴스를 복원하려면 먼저 다른 인스턴스를 삭제하세요.",
  "Its custom domain is now used by another instance, so it cannot be restored.":
    "커스텀 도메인을 다른 인스턴스가 쓰고 있어 복원할 수 없습니다.",
  "Only failed provisioning can be retried.": "설정에 실패한 인스턴스만 다시 시도할 수 있습니다.",
  "The admin account is not ready yet.": "관리자 계정이 아직 준비되지 않았습니다.",
  "Only a running instance can change its domain.":
    "운영 중인 인스턴스만 도메인을 바꿀 수 있습니다.",
  "The instance already has that domain.": "인스턴스가 이미 그 도메인을 쓰고 있습니다.",
  "The instance is already changing its domain.": "인스턴스의 도메인을 이미 바꾸고 있습니다.",
  "Another domain change is running.": "다른 도메인 변경이 진행 중입니다.",
  "No part-way domain change to retry.": "다시 시도할 중단된 도메인 변경이 없습니다.",
  "No domain change is running.": "진행 중인 도메인 변경이 없습니다.",
  "The instance has begun moving and can no longer stop.":
    "인스턴스 이동이 시작되어 더 이상 멈출 수 없습니다.",
  "The domain change was cancelled; the instance stays where it was.":
    "도메인 변경이 취소되었습니다. 인스턴스는 그대로 있습니다.",
  "Enter an SMTP hostname, port 465 or 587, credentials, and a sender email address.":
    "SMTP 호스트 이름, 포트(465 또는 587), 인증 정보, 발신 이메일 주소를 입력하세요.",
  "SMTP settings require a ready instance with an active custom primary domain.":
    "SMTP 설정은 커스텀 도메인이 기본 도메인으로 연결된, 준비된 인스턴스에서만 할 수 있습니다.",
  "The card was declined.": "카드 결제가 거절되었습니다.",
  "Only the account's owner can do that, not an administrator signed in as them.":
    "계정 소유자만 할 수 있습니다. 관리자가 대신 로그인한 상태에서는 할 수 없습니다.",
  "Request could not be completed. Reference: {id}":
    "요청을 처리하지 못했습니다. 참조 번호: {id}",
  "Request failed ({status})": "요청에 실패했습니다 ({status})",
  "Refund amounts": "환불 가능 금액",
  "When you ask": "환불 신청 시점",
  "Amount refunded": "환불 가능 금액",
};
// The catalogs are plain strings, not compiled ones, and Lingui compiles a
// message only in development unless it is given a compiler: without this, a
// production build shows "{domain}" rather than the domain.
i18n.setMessagesCompiler(compileMessage);
i18n.load("en", en);
i18n.load("ko", ko);
export type Locale = "en" | "ko";
const adminPage = location.pathname === "/admin" || location.pathname.startsWith("/admin/");

function apply(locale: Locale) {
  i18n.activate(locale);
  document.documentElement.lang = locale;
  if (!adminPage) document.title = i18n._("eunha space");
}

// A first guess from the browser so nothing renders in the wrong language
// while the server is asked.
apply(
  adminPage ? "en" : navigator.language.toLowerCase().startsWith("ko") ? "ko" : "en",
);

/**
 * The language to start in: a signed-in account's saved one, or what the
 * browser's Accept-Language asks for. A signed-out visitor's own pick on this
 * device wins over the browser's. The console is English only.
 */
export async function initLocale(): Promise<void> {
  if (adminPage) return;
  try {
    // Usually already on its way, asked for by index.html.
    const answer = takeBoot<{ locale: Locale; saved: boolean }>("locale") ??
      fetch("/api/customer/locale").then(async (response) => {
        // An error's body has no locale to activate, and activating none renders nothing.
        if (!response.ok) throw new Error(`Request failed (${response.status})`);
        return (await response.json()) as { locale: Locale; saved: boolean };
      });
    const timeout = new Promise<never>((_, reject) =>
      window.setTimeout(() => reject(new Error("Timed out")), 2000)
    );
    const { locale, saved } = await Promise.race([answer, timeout]);
    if (locale !== "ko" && locale !== "en") throw new Error("No locale");
    const picked = readPicked();
    apply(!saved && picked ? picked : locale);
  } catch {
    const picked = readPicked();
    if (picked) apply(picked);
  }
}

function readPicked(): Locale | null {
  try {
    const value = localStorage.getItem("locale");
    return value === "ko" || value === "en" ? value : null;
  } catch {
    return null;
  }
}

/**
 * Switch language, and keep it: as the account's setting when signed in,
 * and on this device either way.
 */
export async function setLocale(locale: Locale): Promise<void> {
  apply(locale);
  try {
    localStorage.setItem("locale", locale);
  } catch {
    // Private windows may refuse storage; the account's setting still holds.
  }
  await fetch("/api/customer/locale", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ locale }),
  }).catch(() => undefined);
}

export function LocalizationProvider(
  { children }: { children: React.ReactNode },
) {
  return <I18nProvider i18n={i18n}>{children}</I18nProvider>;
}
/** Both languages in one pill, each named in itself, the one in use pressed. */
export function LanguageToggle() {
  const { i18n } = useLingui();
  return (
    <ToggleGroup
      aria-label="Language · 언어"
      spacing={0}
      value={[i18n.locale]}
      onValueChange={(value) => {
        const next = value[0] as Locale | undefined;
        if (next && next !== i18n.locale) void setLocale(next);
      }}
      className="border-border h-9 rounded-full border p-0.5"
    >
      {(["ko", "en"] as const).map((locale) => (
        <ToggleGroupItem
          key={locale}
          value={locale}
          lang={locale}
          aria-label={locale === "ko" ? "한국어" : "English"}
          title={locale === "ko" ? "한국어" : "English"}
          className="h-full min-w-0 rounded-full! px-2.5 text-xs font-semibold"
        >
          {locale.toUpperCase()}
        </ToggleGroupItem>
      ))}
    </ToggleGroup>
  );
}
