if('serviceWorker'in navigator)navigator.serviceWorker.register('/service-worker.js',{scope:'/'}).then(()=>document.documentElement.dataset.swRegistered='true').catch(console.error);
