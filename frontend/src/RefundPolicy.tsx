import { useLingui } from "@lingui/react";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Page, PageHeader, SectionCard } from "@/components/customer/page";

// The refund policy as written in Korean, and its English translation. The
// rates state what `REFUND_WINDOW` in src/billing_sync.rs enforces, and the
// international card entry what `POLAR_SELF_REFUND` does.
const REFUND_RATES_KO = [
  ["결제 후 7일 이내", "결제 금액의 100%"],
  ["결제 후 7일 경과", "환불 불가"],
];

const REFUND_POLICY_KO = [
  [
    "기간 계산",
    "환불 기간은 결제가 완료된 시각부터 계산하며, 정기 결제와 요금제 업그레이드 결제에 각각 적용됩니다. 비율은 서비스 이용 여부와 관계없이 같습니다.",
  ],
  [
    "신청 방법",
    "계정 페이지의 결제 내역에서 직접 신청합니다. 환불 금액은 결제한 카드로 돌아가며, 카드사에 따라 영업일 기준 3~7일이 걸릴 수 있습니다.",
  ],
  [
    "환불 후 이용",
    "정기 결제를 환불하면 구독이 해지되고 인스턴스는 즉시 중단됩니다. 데이터는 보관되며, 다시 구독하면 이어서 이용할 수 있습니다. 요금제 업그레이드 결제를 환불하면 이전 요금제와 요금으로 돌아가며 구독은 계속됩니다. 이전 요금제의 인스턴스 한도를 넘는 인스턴스는 먼저 삭제해야 합니다. 환불은 가장 최근 결제부터 신청할 수 있으며, 한 결제를 환불해도 다른 결제는 함께 환불되지 않습니다.",
  ],
  [
    "해지",
    "정기 결제는 언제든지 해지할 수 있습니다. 다음 결제부터 청구되지 않으며, 이미 결제한 기간은 만료일까지 이용할 수 있습니다. 해지만으로는 환불되지 않으며, 이미 시작된 기간의 일할 환불은 제공하지 않습니다.",
  ],
  [
    "해외 카드",
    "해외 카드 결제는 Polar가 판매자(Merchant of Record)로 처리합니다. 계정 페이지에서 직접 환불을 신청할 수 있는 것은 계정의 첫 해외 카드 결제뿐이며, 이전에 환불을 받은 적이 없는 경우에 한합니다. 그 밖의 결제는 결제 후 7일 이내에 hello@eunha.space로 문의해 주시면 검토 후 환불해 드립니다.",
  ],
  [
    "장애 시 환불",
    "은하공간의 장애나 서비스 중단으로 유료 기능을 이용하지 못한 경우에는 기간과 관계없이 환불해 드립니다. hello@eunha.space로 문의해 주세요.",
  ],
];

const REFUND_RATES_EN = [
  ["Within 7 days of payment", "100% of the amount paid"],
  ["More than 7 days after payment", "No refund"],
];

const REFUND_POLICY_EN = [
  [
    "How the period is counted",
    "The refund period runs from the moment a payment completes, and applies separately to each recurring payment and each plan upgrade payment. The rate is the same whether or not you have used the service.",
  ],
  [
    "How to ask",
    "Ask for a refund yourself from the payment history on your account page. The money goes back to the card you paid with, which can take 3 to 7 business days depending on the card company.",
  ],
  [
    "After a refund",
    "Refunding a recurring payment cancels your subscription and takes your instances offline at once. Their data is kept, and subscribing again lets you carry on. Refunding a plan upgrade returns you to your previous plan and its price, and your subscription goes on. Instances over the previous plan's limit have to be deleted first. Refunds go latest payment first, and refunding one payment never refunds another with it.",
  ],
  [
    "Cancelling",
    "You can cancel a recurring payment at any time. You are not charged again, and you keep the period you already paid for until it ends. Cancelling alone is not a refund, and a period that has already begun is not refunded pro rata.",
  ],
  [
    "International cards",
    "Payments by international card are handled by Polar, our merchant of record. You can ask for a refund yourself from your account page only for your account's first payment by international card, and only if you have not had a refund before. For any other payment, write to hello@eunha.space within 7 days of paying, and we will look at it and refund you where it is fair.",
  ],
  [
    "Refunds for outages",
    "If an outage or interruption of eunha space kept you from using what you paid for, we refund you however long ago you paid. Write to hello@eunha.space.",
  ],
];

/** The refund policy, at `/refund-policy`, for anyone. */
export function RefundPolicy() {
  const { i18n } = useLingui();
  const korean = i18n.locale === "ko";
  const rates = korean ? REFUND_RATES_KO : REFUND_RATES_EN;
  const policy = korean ? REFUND_POLICY_KO : REFUND_POLICY_EN;
  return (
    <Page>
      <PageHeader title={i18n._("Refund policy")} />
      <div className="grid gap-6">
        <SectionCard title={i18n._("Refund amounts")}>
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>{i18n._("When you ask")}</TableHead>
                <TableHead>{i18n._("Amount refunded")}</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {rates.map(([period, rate]) => (
                <TableRow key={period}>
                  <TableCell>{period}</TableCell>
                  <TableCell>{rate}</TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </SectionCard>
        <dl className="grid gap-5">
          {policy.map(([title, body]) => (
            <div key={title}>
              <dt className="font-medium">{title}</dt>
              <dd className="text-muted-foreground mt-1">{body}</dd>
            </div>
          ))}
        </dl>
      </div>
    </Page>
  );
}
