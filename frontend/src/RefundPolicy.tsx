// The refund policy as written, in Korean whatever the interface language.
const REFUND_POLICY = [
  "결제일로부터 7일 이내에는 이유를 묻지 않고 전액 환불해 드립니다. 유료 기능을 이미 쓰셨더라도 괜찮습니다. 결제 내역에서 직접 신청하시면 바로 처리되며, 사유를 적으실 필요도 없습니다.",
  "환불이 완료되면 해당 결제로 제공된 이용 기간이 취소됩니다. 남은 이용 기간이 없으면 최대 4일의 결제 유예 기간 뒤 서비스를 더 이상 이용할 수 없습니다.",
  "정기 결제는 언제든지 해지할 수 있습니다. 해지하시면 다음 결제부터 청구되지 않고, 이미 결제하신 기간은 만료일까지 그대로 이용하실 수 있습니다. 해지는 환불이 아니며, 환불은 결제 내역에서 따로 신청합니다. 이미 시작된 기간에 대한 일할 환불은 제공하지 않습니다. 환불이 이루어지면 정기 결제도 함께 해지되어 더 이상 결제되지 않습니다.",
  "은하공간의 장애나 서비스 중단으로 유료 기능을 이용하지 못한 경우에는 기간 경과와 관계없이 환불해 드립니다",
];

export function RefundPolicy() {
  return (
    <section lang="ko" className="grid gap-2">
      <h3 className="text-sm font-medium">환불정책</h3>
      <ul className="text-muted-foreground grid list-disc gap-1.5 pl-5 text-xs">
        {REFUND_POLICY.map((item) => <li key={item}>{item}</li>)}
      </ul>
    </section>
  );
}
