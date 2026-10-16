import { render } from 'solid-js/web'
import { App } from './App'
import './index.css'

// Apply the persisted theme before first paint to avoid a flash.
const storedTheme = localStorage.getItem('pipeline_theme');
document.documentElement.dataset.theme = storedTheme === 'light' ? 'light' : 'dark';

render(() => <App />, document.getElementById('app')!)
