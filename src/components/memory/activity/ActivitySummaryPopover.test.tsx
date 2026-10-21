// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import ActivityStatus from "./ActivityStatus";
import type { ActivityResponse } from "../../../lib/tauri";
const getActivityMock = vi.hoisted(() => vi.fn());
vi.mock("../../../lib/tauri", () => ({ getActivity: getActivityMock }));
const response: ActivityResponse = {
 state: "blocked", last_activity_at: null,
 everyday: {job:"everyday",lane:"none",model:null,mode:"unconfigured",available:false},
 synthesis: {job:"synthesis",lane:"none",model:null,mode:"unconfigured",available:false},
 refinement:{ready_for_review:0,not_ready:0,groups:[]},
 assets:[
 {kind:"pages",state:"idle",done:2,total:2,blocked:0,steps:[]},
 {kind:"memories",state:"blocked",done:4,total:8,blocked:4,steps:[{name:"store",state:"idle",done:8,total:8,failed:0,job:null}]},
 {kind:"entities",state:"blocked",done:0,total:8,blocked:8,steps:[{name:"detect",state:"blocked",done:0,total:8,failed:0,job:"everyday"},{name:"confirm",state:"idle",done:1,total:3,failed:0,job:null}]},
 ],
};
function setup(data: ActivityResponse | Error = response) {
 if (data instanceof Error) getActivityMock.mockRejectedValue(data);
 else getActivityMock.mockResolvedValue(data);
 const client = new QueryClient({defaultOptions:{queries:{retry:false}}});
 const onOpenActivity=vi.fn();
 render(<QueryClientProvider client={client}><ActivityStatus onOpenActivity={onOpenActivity}/><button>Outside</button></QueryClientProvider>);
 return {client,onOpenActivity};
}
beforeEach(()=>getActivityMock.mockReset());
describe("restored activity summary",()=>{
 it("keeps asset-specific counts and progress without a model settings action",async()=>{
  setup();await userEvent.click(screen.getByTestId("activity-status"));
  await screen.findByTestId("activity-asset-count-memories");
  expect(screen.getByTestId("activity-asset-count-memories")).toHaveTextContent("8");
  expect(screen.getByTestId("activity-asset-count-entities")).toHaveTextContent("3");
  expect(screen.getByTestId("activity-asset-count-pages")).toHaveTextContent("2");
  expect(screen.getByTestId("activity-asset-bar-memories")).toHaveAttribute("data-fraction","0.5");
  expect(screen.getByTestId("activity-asset-bar-entities")).toHaveAttribute("data-fraction","0");
  expect(screen.getByTestId("activity-summary-last")).toHaveTextContent("Nothing has run yet");
  expect(screen.queryByRole("button",{name:/model|settings/i})).toBeNull();
  expect(screen.getByTestId("activity-status-icon")).toHaveAttribute("data-icon-kind","pulse");
 });
 it("keeps counts but omits processing bars when background work is off",async()=>{
  setup({...response,state:"off"});await userEvent.click(screen.getByTestId("activity-status"));
  await screen.findByTestId("activity-asset-count-memories");
  expect(screen.queryByTestId("activity-asset-bar-memories")).toBeNull();
  expect(screen.getByTestId("activity-asset-memories")).toHaveTextContent("8 stored, all searchable now");
  expect(screen.getByTestId("activity-summary-on-demand")).toHaveTextContent("On demand");
 });
 it("closes on Escape with focus returned, and reopens from the keyboard",async()=>{
  setup();const trigger=screen.getByTestId("activity-status");await userEvent.click(trigger);
  await screen.findByTestId("activity-asset-count-memories");
  await userEvent.tab();expect(screen.getByTestId("activity-summary-open")).toHaveFocus();
  await userEvent.keyboard("{Escape}");expect(screen.queryByRole("dialog")).toBeNull();expect(trigger).toHaveFocus();
  await userEvent.keyboard("{Enter}");expect(screen.getByRole("dialog")).toBeVisible();
 });
 it("closes when focus or a pointer leaves without taking focus back",async()=>{
  setup();await userEvent.click(screen.getByTestId("activity-status"));await userEvent.click(screen.getByText("Outside"));
  expect(screen.queryByRole("dialog")).toBeNull();expect(screen.getByText("Outside")).toHaveFocus();
 });
 it("shows errors instead of guessed counts, and can read again",async()=>{
  setup(new Error("offline"));
  await userEvent.click(screen.getByTestId("activity-status"));
  await screen.findByRole("alert");expect(screen.queryByTestId("activity-asset-count-memories")).toBeNull();
  getActivityMock.mockResolvedValue(response);await userEvent.click(screen.getByRole("button",{name:"Read again"}));
  await waitFor(()=>expect(screen.getByTestId("activity-asset-count-memories")).toHaveTextContent("8"));
 });
});
