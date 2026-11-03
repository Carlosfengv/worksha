import type {
  DataTableControlLabels,
  DataTablePaginationLabels,
} from "@/components/ui/data-table";

export const dataTableControls: DataTableControlLabels = {
  ascending: "升序",
  descending: "降序",
  resetSorting: "恢复默认排序",
  hideColumn: "隐藏此列",
  customizeColumns: "选择显示列",
  dragToReorder: "拖动调整列顺序",
  reorderAndToggle: "调整列顺序与显示",
  reorderColumn: (title) => `移动 ${title}`,
  showColumn: (title) => `显示 ${title}`,
};
export const dataTablePagination: DataTablePaginationLabels = {
  firstPage: "第一页",
  lastPage: "最后一页",
  previousPage: "上一页",
  nextPage: "下一页",
  rowsPerPage: "每页条数",
  pageSummary: (page, count) => `第 ${page} / ${count} 页`,
  selectedRows: (selected, total) => `已选 ${selected} / ${total} 项`,
};
