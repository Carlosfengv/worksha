import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";

export function SessionDateFilter({
  value,
  onChange,
}: {
  value: string;
  onChange: (value: string) => void;
}) {
  return (
    <Select value={value} onValueChange={onChange}>
      <SelectTrigger
        aria-label="会话开始日期"
        wrapperClassName="min-w-0 max-w-52"
        className="w-full min-w-0"
      />
      <SelectContent>
        <SelectItem value="all">全部日期</SelectItem>
        <SelectItem value="1">今天</SelectItem>
        <SelectItem value="7">近 7 天</SelectItem>
        <SelectItem value="30">近 30 天</SelectItem>
      </SelectContent>
    </Select>
  );
}
