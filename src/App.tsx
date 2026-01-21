import "./App.css";
import { Tab, Tabs } from "@nextui-org/react";
import { Background } from "./views/background.tsx";
import { BrewServiceHeader } from "./views/brew-service-header.tsx";
import { BrewServiceListView } from "./views/brew-service-list.tsx";
import { LaunchdServiceHeader } from "./views/launchd-service-header.tsx";
import { LaunchdServiceListView } from "./views/launchd-service-list.tsx";

function App() {
	return (
		<Background>
			<Tabs
				aria-label="Service Type"
				variant="underlined"
				classNames={{
					tabList: "gap-6 w-full relative rounded-none p-0 border-b border-divider",
					cursor: "w-full bg-[#22d3ee]",
					tab: "max-w-fit px-0 h-12",
					tabContent: "group-data-[selected=true]:text-[#06b6d4]",
				}}
			>
				<Tab key="homebrew" title="Homebrew">
					<div className="flex flex-col h-full">
						<BrewServiceHeader />
						<BrewServiceListView />
					</div>
				</Tab>
				<Tab key="launchd" title="LaunchAgents">
					<div className="flex flex-col h-full">
						<LaunchdServiceHeader />
						<LaunchdServiceListView />
					</div>
				</Tab>
			</Tabs>
		</Background>
	);
}

export default App;
