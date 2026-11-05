import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";

// The refund policy as written, in Korean whatever the interface language.
// The rates state what `REFUND_WINDOW` in src/billing_sync.rs enforces.
const REFUND_RATES = [
  ["결제 후 7일 이내", "결제 금액의 100%"],
  ["결제 후 7일 경과", "환불 불가"],
];

const REFUND_POLICY = [
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
    "환불하면 정기 결제도 함께 해지됩니다. 지금 이용 중인 기간을 환불하면 4일 뒤 서비스가 중단되고, 이전 기간을 환불하면 이미 결제한 기간이 끝날 때까지 이용할 수 있습니다. 요금제 업그레이드 결제를 환불하면 이전 요금제로 돌아가며, 이미 결제한 기간이 끝날 때까지 이용할 수 있습니다. 서비스가 중단되어도 데이터는 보관되며, 다시 구독하면 이어서 이용할 수 있습니다.",
  ],
  [
    "해지",
    "정기 결제는 언제든지 해지할 수 있습니다. 다음 결제부터 청구되지 않으며, 이미 결제한 기간은 만료일까지 이용할 수 있습니다. 해지만으로는 환불되지 않으며, 이미 시작된 기간의 일할 환불은 제공하지 않습니다.",
  ],
  [
    "장애 시 환불",
    "은하공간의 장애나 서비스 중단으로 유료 기능을 이용하지 못한 경우에는 기간과 관계없이 환불해 드립니다. hello@eunha.space로 문의해 주세요.",
  ],
];

export function RefundPolicy() {
  return (
    <section lang="ko" className="grid gap-2">
      <h3 className="text-sm font-medium">환불정책</h3>
      <Table className="text-xs">
        <TableHeader>
          <TableRow>
            <TableHead>환불 신청 시점</TableHead>
            <TableHead>환불 가능 금액</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {REFUND_RATES.map(([period, rate]) => (
            <TableRow key={period}>
              <TableCell>{period}</TableCell>
              <TableCell>{rate}</TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
      <ul className="text-muted-foreground grid list-disc gap-1.5 pl-5 text-xs">
        {REFUND_POLICY.map(([title, body]) => (
          <li key={title}>
            <strong className="text-foreground font-medium">{title}:</strong> {body}
          </li>
        ))}
      </ul>
    </section>
  );
}
