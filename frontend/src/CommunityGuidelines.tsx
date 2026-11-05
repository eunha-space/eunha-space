import type { ReactNode } from "react";
import { useLingui } from "@lingui/react";
import { Page, PageHeader } from "@/components/customer/page";
import { CONTACT_EMAIL } from "@/planLimits";

/**
 * The community guidelines, at `/community-guidelines`, for anyone. Written in
 * Korean after naru.pub's, with an English translation; the Korean is the one
 * that holds.
 */
export function CommunityGuidelines() {
  const { i18n } = useLingui();
  const korean = i18n.locale === "ko";
  return (
    <Page>
      <PageHeader
        title={i18n._("Community guidelines")}
        description={korean ? "시행일 2026년 11월 6일" : "In effect from November 6, 2026"}
      />
      <div className="grid gap-8">{korean ? <Korean /> : <English />}</div>
    </Page>
  );
}

function Korean() {
  return (
    <>
      <p className="text-muted-foreground">
        은하공간은 누구나 마스토돈과 호환되는 자신만의 서버를 띄울 수 있는
        호스팅입니다. 서로의 서버를 존중하고, 연합우주 어디서든 안심하고 머물 수
        있도록 아래 약속을 지켜주세요. 이 가이드라인은 은하공간 계정과 은하공간에서
        운영되는 모든 인스턴스, 그 인스턴스의 계정이 올린 게시물과 미디어, 다른
        서버로 전달되는 활동에 적용됩니다.
      </p>

      <Section title="금지하는 콘텐츠와 행동">
        <List>
          <li>다른 사람에 대한 괴롭힘, 위협, 따돌림, 스토킹</li>
          <li>
            인종, 민족, 국적, 종교, 성별, 성적 지향, 성별 정체성, 장애, 나이 등을
            이유로 한 혐오 표현이나 차별
          </li>
          <li>
            아동·청소년이 등장하는 성적 콘텐츠, 동의 없이 촬영하거나 퍼뜨리는 성적
            콘텐츠, 딥페이크 등 합성 성적 콘텐츠, 음란물
          </li>
          <li>
            폭력이나 자해의 조장, 마약·불법 도박·불법 무기 등의 거래나 홍보, 그 밖에
            법령을 어기는 콘텐츠와 활동
          </li>
          <li>
            다른 사람의 저작권, 상표권 등 권리를 침해하는 콘텐츠. 직접 만들었거나 쓸
            권리가 있는 것만 올려주세요.
          </li>
          <li>피싱, 악성코드나 그 배포, 다른 서비스를 사칭해 정보를 빼내는 행위</li>
          <li>스팸, 광고 도배, 다른 서버에 대한 대량 팔로우·멘션·답글</li>
          <li>다른 사람이나 단체를 사칭하는 행위, 신원 사기</li>
          <li>
            다른 사람의 개인정보(실명, 연락처, 주소, 사진 등)를 동의 없이 공개하는
            행위
          </li>
          <li>사실이 아닌 내용으로 다른 사람의 명예를 훼손하는 행위</li>
        </List>
      </Section>

      <Section title="서비스를 아껴 써주세요">
        <List>
          <li>
            은하공간은 소셜 서버를 위한 공간입니다. 게시물과 관계없는 대용량 파일을
            올려 두고 다른 곳에서 내려받게 하는 파일 저장소로 쓰지 말아주세요.
          </li>
          <li>
            암호화폐 채굴, 다른 서비스에 대한 공격, 자동화된 대량 요청처럼 은하공간이나
            다른 서버에 부담을 주는 용도로 쓰지 말아주세요.
          </li>
          <li>
            요금제의 사용자 수, 저장 용량, 월 ActivityPub 활동 등 은하공간의 제한을
            우회하거나 다른 사람의 계정에 무단으로 접근하지 말아주세요.
          </li>
          <li>계정이나 인스턴스를 사고팔거나 다른 사람에게 넘기지 말아주세요.</li>
        </List>
      </Section>

      <Section title="인스턴스 운영자의 책임">
        <List>
          <li>
            인스턴스에 올라오는 내용은, 회원이 올린 게시물을 포함해 인스턴스 운영자가
            관리합니다. 회원이 이 가이드라인을 어기면 은하의 관리 도구로 게시물을
            지우거나 계정을 제한해 주세요.
          </li>
          <li>
            회원 가입을 열면 누가 가입하는지 살펴봐 주세요. 서버 규칙을 정해 두면
            회원과 방문자 모두에게 도움이 됩니다.
          </li>
          <li>
            회원이나 다른 서버에서 온 신고를 확인하고, 문제가 되는 내용에는 조치해
            주세요.
          </li>
          <li>
            회원의 개인정보를 다룬다면 무엇을 왜 받는지 회원에게 알리고, 관련 법령을
            지켜주세요.
          </li>
        </List>
      </Section>

      <Section title="자동화와 AI">
        <List>
          <li>자동으로 글을 올리는 계정은 프로필에서 봇 계정으로 표시해 주세요.</li>
          <li>AI로 만든 게시물이나 이미지는 그렇다고 밝혀주세요.</li>
          <li>다른 사람의 창작물을 흉내 내지 말아주세요.</li>
        </List>
      </Section>

      <Section title="위반 시 조치">
        <p>
          가이드라인을 어긴 콘텐츠는 인스턴스 운영자에게 삭제를 요청하거나, 알림 없이
          직접 삭제하거나 비공개로 전환할 수 있습니다. 위반의 정도에 따라 경고, 일부
          기능 제한, 인스턴스의 일시 중단이나 영구 정지를 할 수 있으며, 아동
          성착취물이나 피싱처럼 심각한 위반은 경고 없이 즉시 영구 정지합니다. 불법
          행위는 수사기관에 신고할 수 있습니다.
        </p>
        <p>
          조치에 이의가 있으시면 <Mail />로 알려주세요. 다시 검토한 뒤 답변드립니다.
        </p>
      </Section>

      <Section title="신고">
        <p>
          은하공간의 인스턴스에서 가이드라인을 어기는 게시물이나 계정을 발견하시면,
          먼저 그 인스턴스의 신고 기능을 써주세요. 운영자가 조치하지 않거나 인스턴스
          자체가 문제라면 문제가 되는 주소와 이유를 <Mail />로 보내주세요. 모든 신고를
          확인하고 빠르게 처리합니다. 신고하신 분의 정보는 신고 처리 외에 쓰지 않으며
          신고 대상에게 알리지 않습니다.
        </p>
        <p>
          저작권 침해나 명예훼손 등으로 권리를 침해받으셨다면, 침해된 권리와 침해하는
          콘텐츠의 주소, 권리자임을 확인할 수 있는 자료를 함께 보내주세요.
          「정보통신망 이용촉진 및 정보보호 등에 관한 법률」에 따라 해당 콘텐츠를
          삭제하거나 임시로 접근을 막고, 결과를 알려드립니다.
        </p>
      </Section>

      <Section title="가이드라인의 변경">
        <p>
          은하공간을 이용하시면 이 가이드라인에 동의하신 것으로 봅니다. 가이드라인은
          서비스와 함께 바뀔 수 있으며, 바뀌면 이 페이지에 시행일과 함께 알립니다.
        </p>
      </Section>
    </>
  );
}

function English() {
  return (
    <>
      <p className="text-muted-foreground">
        eunha space is hosting where anyone can run a Mastodon-compatible server of
        their own. Respect each other's servers, and help keep the fediverse a place
        where everyone can stay safely, by keeping to the promises below. These
        guidelines apply to eunha space accounts and every instance run on eunha
        space: the posts and media its accounts put up, and the activity it sends to
        other servers.
      </p>

      <Section title="What is not allowed">
        <List>
          <li>Harassing, threatening, excluding or stalking anyone</li>
          <li>
            Hate speech or discrimination over race, ethnicity, nationality,
            religion, sex, sexual orientation, gender identity, disability, age and
            the like
          </li>
          <li>
            Sexual content involving minors, sexual content recorded or spread
            without consent, synthetic sexual content such as deepfakes, and obscene
            material
          </li>
          <li>
            Encouraging violence or self-harm, trading or promoting drugs, illegal
            gambling or illegal weapons, and any other content or activity that
            breaks the law
          </li>
          <li>
            Content that infringes others' copyright, trademarks or other rights.
            Only post what you made or have the right to use.
          </li>
          <li>
            Phishing, malware or spreading it, and impersonating another service to
            take people's information
          </li>
          <li>Spam, advertising floods, and mass follows, mentions or replies to other servers</li>
          <li>Impersonating a person or organization, and identity fraud</li>
          <li>
            Publishing someone's personal information (real name, contact details,
            address, photos and the like) without their consent
          </li>
          <li>Defaming someone with things that are not true</li>
        </List>
      </Section>

      <Section title="Use the service with care">
        <List>
          <li>
            eunha space is for social servers. Don't use it as file storage, putting
            up large files unrelated to posts for people to download elsewhere.
          </li>
          <li>
            Don't use it in ways that burden eunha space or other servers, such as
            mining cryptocurrency, attacking other services or sending automated
            requests in bulk.
          </li>
          <li>
            Don't get around eunha space's limits, such as your plan's users,
            storage or monthly ActivityPub activities, and don't get into other
            people's accounts without permission.
          </li>
          <li>Don't buy, sell or hand over accounts or instances.</li>
        </List>
      </Section>

      <Section title="What an instance's owner is responsible for">
        <List>
          <li>
            What appears on an instance, members' posts included, is its owner's to
            look after. When a member breaks these guidelines, remove the posts or
            limit the account with Eunha's moderation tools.
          </li>
          <li>
            If you open registrations, keep an eye on who joins. Server rules help
            members and visitors alike.
          </li>
          <li>Read the reports from members and other servers, and act on what needs it.</li>
          <li>
            If you handle members' personal information, tell them what you collect
            and why, and follow the law that applies.
          </li>
        </List>
      </Section>

      <Section title="Automation and AI">
        <List>
          <li>Mark accounts that post automatically as bots on their profiles.</li>
          <li>Say so when a post or image was made with AI.</li>
          <li>Don't imitate other people's creative work.</li>
        </List>
      </Section>

      <Section title="What happens when the guidelines are broken">
        <p>
          We may ask an instance's owner to remove content that breaks these
          guidelines, or remove it or make it private ourselves without notice.
          Depending on how serious it is, we may warn, limit some features, or
          suspend or permanently close an instance. Serious violations such as child
          sexual abuse material or phishing are closed permanently at once, without
          warning. Illegal activity may be reported to the authorities.
        </p>
        <p>
          If you disagree with an action, write to <Mail />. We will look at it again
          and reply.
        </p>
      </Section>

      <Section title="Reporting">
        <p>
          If you find a post or account on an eunha space instance that breaks these
          guidelines, use that instance's report feature first. If its owner does not
          act, or the instance itself is the problem, send the address and the reason
          to <Mail />. We read every report and deal with it quickly. What we learn
          about you is used only to handle the report and is not shared with whoever
          you reported.
        </p>
        <p>
          If your rights were infringed, for example by copyright infringement or
          defamation, send the right infringed, the address of the content, and
          something that shows the right is yours. Under Korea's Act on Promotion of
          Information and Communications Network Utilization and Information
          Protection, we remove the content or block access to it for a time, and tell
          you the outcome.
        </p>
      </Section>

      <Section title="Changes to these guidelines">
        <p>
          Using eunha space means you agree to these guidelines. They may change with
          the service; when they do, this page says so with the date they take
          effect. The Korean text is the one that holds.
        </p>
      </Section>
    </>
  );
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="grid gap-3">
      <h2 className="text-xl font-semibold tracking-tight">{title}</h2>
      <div className="text-muted-foreground grid gap-3">{children}</div>
    </section>
  );
}

function List({ children }: { children: ReactNode }) {
  return <ul className="grid list-disc gap-1.5 pl-5">{children}</ul>;
}

function Mail() {
  return (
    <a href={`mailto:${CONTACT_EMAIL}`} className="text-foreground underline underline-offset-4">
      {CONTACT_EMAIL}
    </a>
  );
}
