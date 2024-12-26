import "./App.css";
import { Background } from "./views/background.tsx";
import { BrewServiceListView } from "./views/brew-service-list.tsx";

function App() {
	return (
		<Background>
			<>
				<div className="mb-4 flex flex-nowrap items-center justify-between">
					<p className="relative bg-gradient-to-b from-neutral-200 to-neutral-500 bg-clip-text font-bold text-transparent">
						Homebrew Services
					</p>
					<div>button</div>
				</div>
				<BrewServiceListView />
			</>
		</Background>
	);
}

export default App;
