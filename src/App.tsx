import "./App.css";
import { Background } from "./views/background.tsx";
import { BrewServiceHeader } from "./views/brew-service-header.tsx";
import { BrewServiceListView } from "./views/brew-service-list.tsx";

function App() {
	return (
		<Background>
			<>
				<BrewServiceHeader />
				<BrewServiceListView />
			</>
		</Background>
	);
}

export default App;
